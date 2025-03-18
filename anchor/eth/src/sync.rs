use crate::error::ExecutionError;
use crate::event_processor::EventProcessor;
use crate::gen::SSVContract;
use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder, RootProvider, WsConnect};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use database::NetworkDatabase;
use futures::future::{try_join_all, Future};
use futures::StreamExt;
use reqwest::Url;
use ssv_network_config::SsvNetworkConfig;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::sync::oneshot::Sender;
use tokio::time::Duration;
use tracing::{debug, error, info, instrument, warn};

/// SSV contract events needed to come up to date with the network
static SSV_EVENTS: LazyLock<Vec<String>> = LazyLock::new(|| {
    vec![
        // event OperatorAdded(uint64 indexed operatorId, address indexed owner, bytes publicKey, uint256 fee);
        SSVContract::OperatorAdded::SIGNATURE.to_string(),
        // event OperatorRemoved(uint64 indexed operatorId);
        SSVContract::OperatorRemoved::SIGNATURE.to_string(),
        // event ValidatorAdded(address indexed owner, uint64[] operatorIds, bytes publicKey, bytes shares, Cluster cluster);
        SSVContract::ValidatorAdded::SIGNATURE.to_string(),
        // event ValidatorRemoved(address indexed owner, uint64[] operatorIds, bytes publicKey, Cluster cluster);
        SSVContract::ValidatorRemoved::SIGNATURE.to_string(),
        // event ClusterLiquidated(address indexed owner, uint64[] operatorIds, Cluster cluster);
        SSVContract::ClusterLiquidated::SIGNATURE.to_string(),
        // event ClusterReactivated(address indexed owner, uint64[] operatorIds, Cluster cluster);
        SSVContract::ClusterReactivated::SIGNATURE.to_string(),
        // event FeeRecipientAddressUpdated(address indexed owner, address recipientAddress);
        SSVContract::FeeRecipientAddressUpdated::SIGNATURE.to_string(),
        // event ValidatorExited(address indexed owner, uint64[] operatorIds, bytes publicKey);
        SSVContract::ValidatorExited::SIGNATURE.to_string(),
    ]
});

/// SSV contract events that provide information for keysplitting
static KEYSPLIT_EVENTS: LazyLock<Vec<String>> = LazyLock::new(|| {
    vec![
        // Provides operator information
        SSVContract::OperatorAdded::SIGNATURE.to_string(),
        // Provides nonce information
        SSVContract::ValidatorAdded::SIGNATURE.to_string(),
    ]
});

/// Batch size for log fetching
const BATCH_SIZE: u64 = 10000;

/// Batch size for task groups
const GROUP_SIZE: usize = 50;

/// Exponential backoff constants
const INITIAL_BACKOFF_MS: u64 = 100; // Start with 100ms delay
const MAX_BACKOFF_MS: u64 = 30_000; // Don't wait longer than 30 seconds

// Block follow distance
const FOLLOW_DISTANCE: u64 = 8;

/// The maximum number of operators a validator can have
/// https://github.com/ssvlabs/ssv/blob/07095fe31e3ded288af722a9c521117980585d95/eth/eventhandler/validation.go#L15
pub const MAX_OPERATORS: usize = 13;

// TODO!() Dummy config struct that will be replaced
#[derive(Debug)]
pub struct Config {
    pub http_url: String,
    pub ws_url: String,
    pub beacon_url: String,
    pub network: SsvNetworkConfig,
    pub historic_finished_notify: Option<Sender<()>>,
}

/// Client for interacting with the SSV contract on Ethereum L1
///
/// Manages connections to the L1 and monitors SSV contract events to track the state of validator
/// and operators. Provides both historical synchronization and live event monitoring
pub struct SsvEventSyncer {
    /// Http client connected to the L1 to fetch historical SSV event information
    rpc_client: Arc<RootProvider>,
    /// Websocket client connected to L1 to stream live SSV event information
    ws_client: RootProvider,
    /// Websocket connection url
    ws_url: String,
    /// Event processor for logs
    event_processor: EventProcessor,
    /// The network the node is connected to
    network: SsvNetworkConfig,
    /// Notify a channel as soon as the historical sync is done
    historic_finished_notify: Option<Sender<()>>,
    /// Current operational status of sync. If there is an issue with the rpc endpoint or the ws
    /// endpoint, the status is considered down. Otherwise, it is up
    operational_status: Arc<AtomicBool>,
}

impl SsvEventSyncer {
    #[instrument(skip(db, config))]
    /// Create a new SsvEventSyncer to sync all of the events from the chain
    pub async fn new(db: Arc<NetworkDatabase>, config: Config) -> Result<Self, ExecutionError> {
        info!(?config, "Creating new SSV Event Syncer");

        // Construct HTTP Provider
        let http_url = config.http_url.parse().expect("Failed to parse HTTP URL");
        let rpc_client = Arc::new(ProviderBuilder::default().on_http(http_url));

        // Construct Websocket Provider
        let ws = WsConnect::new(&config.ws_url);
        let ws_client = ProviderBuilder::default()
            .on_ws(ws.clone())
            .await
            .map_err(|e| {
                ExecutionError::SyncError(format!(
                    "Failed to bind to WS: {}, {}",
                    &config.ws_url, e
                ))
            })?;

        // Construct an EventProcessor with access to the DB
        let event_processor = EventProcessor::new(db, false);

        Ok(Self {
            rpc_client,
            ws_client,
            ws_url: config.ws_url,
            event_processor,
            network: config.network,
            historic_finished_notify: config.historic_finished_notify,
            operational_status: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Create a new event syncer for a keysplit sync
    pub fn new_keysplit(db: Arc<NetworkDatabase>, rpc_endpoint: String, network: String) -> Self {
        let http_url: Url = rpc_endpoint.parse().expect("Failed to parse HTTP URL");
        let rpc_client = Arc::new(ProviderBuilder::default().on_http(http_url.clone()));

        let event_processor = EventProcessor::new(db, true);

        // The network is enforced to be either "mainnet" or "holesky" so this will never fail.
        let network = match SsvNetworkConfig::constant(&network) {
            Ok(Some(net)) => net,
            // These cases should be unreachable due to type constraints, but we handle them explicitly
            Ok(None) => panic!("Network configuration unexpectedly empty"),
            Err(e) => panic!("Invalid network configuration: {}", e),
        };

        // This does not perform a live sync, so we just want to mock websocket fields. This helps
        // so that we dont have to switch the ws fields to Option and clutter up the rest of the
        // application unnecessarily
        let ws_url = String::from("");
        let ws_client = ProviderBuilder::default().on_http(http_url);

        Self {
            rpc_client,
            ws_client,
            ws_url,
            event_processor,
            network,
            historic_finished_notify: None,
            operational_status: Arc::new(AtomicBool::new(false)),
        }
    }

    // Perform a historical keysplit sync. A keysplit sync is a normal sync that only fetches
    // OperatorAdded and Validator Added events
    pub async fn keysplit_sync(&mut self) {
        let contract_address = self.network.ssv_contract;
        let deployment_block = self.network.ssv_contract_block;

        // Historical sync with disconnect handling
        loop {
            match self
                .historical_sync(contract_address, deployment_block, KEYSPLIT_EVENTS.clone())
                .await
            {
                Ok(_) => return,
                Err(e) => {
                    error!(?e, "Sync failed, attempting recovery");
                    if let ExecutionError::RpcError(e) = e {
                        warn!("Rpc error: {e}");
                        self.troubleshoot_rpc().await
                    }
                }
            }
        }
    }

    // Get access to the current status of the sync
    pub fn operational_status(&self) -> Arc<AtomicBool> {
        self.operational_status.clone()
    }

    #[instrument(skip(self))]
    /// Try to perform both a historical and live sync from the chain
    pub async fn sync(&mut self) -> Result<(), ExecutionError> {
        info!("Starting SSV event sync");
        // Get network specific contract information
        let contract_address = self.network.ssv_contract;
        let deployment_block = self.network.ssv_contract_block;

        info!(
            ?contract_address,
            deployment_block, "Using contract configuration"
        );
        loop {
            match self.try_sync(contract_address, deployment_block).await {
                Ok(_) => unreachable!("Sync should never finish successfully"),
                Err(e) => {
                    error!(?e, "Sync failed, attempting recovery");
                    self.operational_status.store(false, Ordering::Relaxed);

                    match e {
                        ExecutionError::WsError(e) => {
                            warn!("Websocket error: {e}");
                            self.troubleshoot_ws().await;
                        }
                        ExecutionError::RpcError(e) => {
                            warn!("Rpc error: {e}");
                            self.troubleshoot_rpc().await
                        }
                        _ => {} // these are logged where they occur
                    }

                    self.operational_status.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    // When we encounter a rpc error, keep polling until success
    async fn troubleshoot_rpc(&self) {
        info!("Attempting to reconnect to rpc");
        let mut retry_count = 0;
        let mut current_backoff_ms = INITIAL_BACKOFF_MS;

        while (self.rpc_client.get_block_number().await).is_err() {
            self.apply_backoff(&mut retry_count, &mut current_backoff_ms)
                .await;
        }
    }

    // When we encounter a ws error, keep trying to connect until success
    pub async fn troubleshoot_ws(&mut self) {
        info!("Attempting to reconnect to ws");
        let mut retry_count = 0;
        let mut current_backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            let ws = WsConnect::new(&self.ws_url);
            if let Ok(ws_client) = ProviderBuilder::default().on_ws(ws).await {
                self.ws_client = ws_client;
                break;
            }
            // unsuccessfull, backoff
            self.apply_backoff(&mut retry_count, &mut current_backoff_ms)
                .await;
        }
    }

    // Exponential backoff with cap
    pub async fn apply_backoff(&self, retry_count: &mut i32, current_backoff_ms: &mut u64) {
        // Calculate next backoff with some jitter
        let jitter = fastrand::u64(0..=50); // Random 0-50ms
        *current_backoff_ms = (*current_backoff_ms * 2) // Exponential growth
            .min(MAX_BACKOFF_MS) // Don't exceed max backoff
            .saturating_add(jitter); // Add jitter safely

        warn!(
            retry_count,
            backoff_ms = current_backoff_ms,
            "Conneciton error, backing off before retry"
        );
        *retry_count += 1;

        tokio::time::sleep(Duration::from_millis(*current_backoff_ms)).await;
    }

    #[instrument(skip(self))]
    /// Initial both a historical sync and a live sync from the chain. This function will transition
    /// into a never ending live sync, so it should never return
    pub async fn try_sync(
        &mut self,
        contract_address: Address,
        deployment_block: u64,
    ) -> Result<(), ExecutionError> {
        info!("Starting historical sync");
        self.historical_sync(contract_address, deployment_block, SSV_EVENTS.clone())
            .await?;

        self.historic_finished_notify.take().map(|x| x.send(()));

        info!("Starting live sync");
        self.live_sync(contract_address).await?;

        // If we reach there, there is some non-recoverable error and we should shut down
        Err(ExecutionError::SyncError(
            "Sync has unexpectedly exited".to_string(),
        ))
    }

    // Perform a historical sync on the network. This will fetch blocks from the contract deployment
    // block up until the current tip of the chain. This way, we can recreate the current state of
    // the network through event logs
    #[instrument(skip(self, contract_address, deployment_block, events))]
    async fn historical_sync(
        &self,
        contract_address: Address,
        deployment_block: u64,
        events: Vec<String>,
    ) -> Result<(), ExecutionError> {
        // Start from the contract deployment block or the last block that has been processed
        let last_processed_block = self.event_processor.db.state().get_last_processed_block();
        let mut start_block = std::cmp::max(deployment_block, last_processed_block + 1);

        loop {
            let current_block = self.rpc_client.get_block_number().await.map_err(|e| {
                error!(?e, "Failed to fetch block number");
                ExecutionError::RpcError(format!("Failed to fetch block number: {e}"))
            })?;

            // Basic verification
            if current_block < FOLLOW_DISTANCE {
                debug!("Current block less than follow distance, breaking");
                break;
            }
            let end_block = current_block - FOLLOW_DISTANCE;
            if end_block < start_block {
                debug!("End block less than start block, breaking");
                break;
            }

            // Make sure we have blocks to sync
            if start_block == end_block && start_block - 1 != last_processed_block {
                info!("Synced up to the tip of the chain, breaking");
                break;
            }

            // Here, we have a start..end block that we need to sync the logs from. This range gets
            // broken up into individual ranges of BATCH_SIZE where the logs are fetches from. The
            // individual ranges are further broken up into a set of batches that are sequentually
            // processes. This makes it so we dont have a ton of logs that all have to be processed
            // in one pass

            // Chunk the start and end block range into a set of ranges of size BATCH_SIZE
            // and construct a future to fetch the logs in each range
            let mut tasks: Vec<_> = (start_block..=end_block)
                .step_by(BATCH_SIZE as usize)
                .map(|start| {
                    let (start, end) = (start, std::cmp::min(start + BATCH_SIZE - 1, end_block));
                    self.fetch_logs(start, end, contract_address, events.clone())
                })
                .collect();

            // Further chunk the block ranges into groups where each group covers 500k blocks, so
            // there are 50 tasks per group. BATCH_SIZE * 50 = 500k
            let mut task_groups = Vec::new();
            while !tasks.is_empty() {
                // Drain takes elements from the original vector, moving them to a new vector
                // take up to chunk_size elements (or whatever is left if less than chunk_size)
                let chunk: Vec<_> = tasks.drain(..tasks.len().min(GROUP_SIZE)).collect();
                task_groups.push(chunk);
            }

            info!(
                start_block = start_block,
                end_block = end_block,
                "Syncing all events"
            );
            for (index, group) in task_groups.into_iter().enumerate() {
                let calculated_start =
                    start_block + (index as u64 * BATCH_SIZE * GROUP_SIZE as u64);
                let calculated_end = calculated_start + (BATCH_SIZE * GROUP_SIZE as u64) - 1;
                let calculated_end = std::cmp::min(calculated_end, end_block);
                info!(
                    "Fetching logs for block range {}..{}",
                    calculated_start, calculated_end
                );

                // Await all of the futures.
                let event_logs: Vec<Vec<Log>> = try_join_all(group).await.map_err(|e| {
                    ExecutionError::RpcError(format!("Failed to join log future: {e}"))
                })?;
                let event_logs: Vec<Log> = event_logs.into_iter().flatten().collect();

                // The futures may join out of order block wise. The individual events within the block
                // retain their tx ordering. Due to this, we can reassemble back into blocks and be
                // confident the order is correct
                let mut ordered_event_logs: BTreeMap<u64, Vec<Log>> = BTreeMap::new();
                for log in event_logs {
                    let block_num = log
                        .block_number
                        .ok_or("Log is missing block number")
                        .map_err(|e| {
                            ExecutionError::RpcError(format!("Failed to fetch block number: {e}"))
                        })?;
                    ordered_event_logs.entry(block_num).or_default().push(log);
                }
                let ordered_event_logs: Vec<Log> =
                    ordered_event_logs.into_values().flatten().collect();

                // Logs are all fetched from the chain and in order, process them but do not send off to
                // be processed since we are just reconstructing state
                self.event_processor.process_logs(ordered_event_logs, false);

                // Record that we have processed up to this block
                self.event_processor
                    .db
                    .processed_block(calculated_end)
                    .expect("Failed to update last processed block number");
            }

            info!("Processed all events up to block {}", end_block);

            // update end block processed information
            start_block = end_block + 1;
        }
        info!("Historical sync completed");
        Ok(())
    }

    // Construct a future that will fetch logs in the range from_block..to_block
    #[instrument(skip(self, deployment_address, events))]
    fn fetch_logs(
        &self,
        from_block: u64,
        to_block: u64,
        deployment_address: Address,
        events: Vec<String>,
    ) -> impl Future<Output = Result<Vec<Log>, ExecutionError>> + use<'_> {
        // Setup filter and rpc client
        let rpc_client = self.rpc_client.clone();
        let filter = Filter::new()
            .address(deployment_address)
            .from_block(from_block)
            .to_block(to_block)
            .events(&events);

        // Try to fetch logs with a retry upon error. Try up to MAX_RETRIES times and error if we
        // exceed this as we can assume there is some underlying connection issue
        async move {
            match rpc_client.get_logs(&filter).await {
                Ok(logs) => {
                    debug!(log_count = logs.len(), "Successfully fetched logs");
                    Ok(logs)
                }
                Err(e) => Err(ExecutionError::RpcError(format!(
                    "Error fetching logs: {e}"
                ))),
            }
        }
    }

    // Once caught up with the chain, start live sync which will stream in live blocks from the
    // network. The events will be processed and duties will be created in response to network
    // actions
    #[instrument(skip(self, contract_address))]
    async fn live_sync(&mut self, contract_address: Address) -> Result<(), ExecutionError> {
        info!("Network up to sync..");
        info!("Current state");
        info!(?contract_address, "Starting live sync");

        loop {
            // Try to subscribe to a block stream
            let stream = match self.ws_client.subscribe_blocks().await {
                Ok(sub) => {
                    info!("Successfully subscribed to block stream");
                    Some(sub.into_stream())
                }
                Err(e) => {
                    return Err(ExecutionError::WsError(format!(
                        "Failed to subscribe to block stream: {e}"
                    )));
                }
            };

            // If we have a connection, continuously stream in blocks
            if let Some(mut stream) = stream {
                while let Some(block_header) = stream.next().await {
                    // Block we are interested in is the current block number - follow distance
                    let relevant_block = block_header.number - FOLLOW_DISTANCE;
                    debug!(
                        block_number = block_header.number,
                        relevant_block, "Processing new block"
                    );

                    let logs = self
                        .fetch_logs(
                            relevant_block,
                            relevant_block,
                            contract_address,
                            SSV_EVENTS.clone(),
                        )
                        .await?;

                    info!(
                        log_count = logs.len(),
                        "Processing events from block {}", relevant_block
                    );

                    // process the logs and update the last block we have recorded
                    self.event_processor.process_logs(logs, true);
                    self.event_processor
                        .db
                        .processed_block(relevant_block)
                        .expect("Failed to update last processed block number");
                }
            }

            // If we get here, the stream ended (likely due to disconnect)
            error!("WebSocket stream ended, reconnecting...");
        }
    }
}
