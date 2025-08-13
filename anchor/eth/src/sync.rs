use std::{
    cmp::{max, min},
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use alloy::{
    eips::BlockNumberOrTag,
    primitives::Address,
    providers::{Provider, ProviderBuilder, RootProvider, WsConnect},
    rpc::types::{Filter, Log},
    sol_types::SolEvent,
    transports::{RpcError, TransportErrorKind},
};
use database::NetworkDatabase;
use futures::{FutureExt, StreamExt, stream::FuturesOrdered};
use reqwest::Url;
use sensitive_url::SensitiveUrl;
use slashing_protection::SlashingDatabase;
use ssv_network_config::SsvNetworkConfig;
use tokio::{select, sync::watch, task::spawn_blocking, time::Duration};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::{
    error::ExecutionError,
    event_processor::{EventProcessor, Mode},
    generated::SSVContract,
    index_sync, metrics,
    util::http_with_timeout_and_fallback,
    voluntary_exit_processor::ExitTx,
};

/// SSV contract events needed to come up to date with the network
const SSV_EVENTS: &[&str] = &[
    // event OperatorAdded(uint64 indexed operatorId, address indexed owner, bytes publicKey,
    // uint256 fee);
    SSVContract::OperatorAdded::SIGNATURE,
    // event OperatorRemoved(uint64 indexed operatorId);
    SSVContract::OperatorRemoved::SIGNATURE,
    // event ValidatorAdded(address indexed owner, uint64[] operatorIds, bytes publicKey, bytes
    // shares, Cluster cluster);
    SSVContract::ValidatorAdded::SIGNATURE,
    // event ValidatorRemoved(address indexed owner, uint64[] operatorIds, bytes publicKey,
    // Cluster cluster);
    SSVContract::ValidatorRemoved::SIGNATURE,
    // event ClusterLiquidated(address indexed owner, uint64[] operatorIds, Cluster cluster);
    SSVContract::ClusterLiquidated::SIGNATURE,
    // event ClusterReactivated(address indexed owner, uint64[] operatorIds, Cluster cluster);
    SSVContract::ClusterReactivated::SIGNATURE,
    // event FeeRecipientAddressUpdated(address indexed owner, address recipientAddress);
    SSVContract::FeeRecipientAddressUpdated::SIGNATURE,
    // event ValidatorExited(address indexed owner, uint64[] operatorIds, bytes publicKey);
    SSVContract::ValidatorExited::SIGNATURE,
];

/// SSV contract events that provide information for keysplitting
const KEYSPLIT_EVENTS: &[&str] = &[
    // Provides operator information
    SSVContract::OperatorAdded::SIGNATURE,
    // Provides nonce information
    SSVContract::ValidatorAdded::SIGNATURE,
];

/// Batch size for log fetching
const BATCH_SIZE: u64 = 10000;

/// Log after this many batches have been processed
const LOG_AFTER_BATCHES: u64 = 50;

/// Number of batches to fetch concurrently
const FETCH_CONCURRENT: usize = 50;

/// Exponential backoff constants
const INITIAL_BACKOFF_MS: u64 = 100; // Start with 100ms delay
const MAX_BACKOFF_MS: u64 = 30_000; // Don't wait longer than 30 seconds

// Block follow distance
const FOLLOW_DISTANCE: u64 = 8;

// Connection timeout duration
pub const CONNECT_TIMEOUT: u64 = 10;

/// The maximum number of operators a validator can have
/// https://github.com/ssvlabs/ssv/blob/07095fe31e3ded288af722a9c521117980585d95/eth/eventhandler/validation.go#L15
pub const MAX_OPERATORS: usize = 13;

// TODO: allow specification of multiple URLs
#[derive(Debug)]
pub struct Config {
    pub http_urls: Vec<SensitiveUrl>,
    pub ws_url: SensitiveUrl,
    pub network: SsvNetworkConfig,
}

/// Client for interacting with the SSV contract on Ethereum L1
///
/// Manages connections to the L1 and monitors SSV contract events to track the state of validator
/// and operators. Provides both historical synchronization and live event monitoring
pub struct SsvEventSyncer {
    /// Http client connected to the L1 to fetch historical SSV event information
    rpc_client: RootProvider,
    /// Websocket client connected to L1 to stream live SSV event information
    ws_client: RootProvider,
    /// Websocket connection url
    ws_url: String,
    /// Event processor for logs
    event_processor: EventProcessor,
    /// The network the node is connected to
    network: SsvNetworkConfig,
    /// Current sync status
    is_synced: watch::Sender<bool>,
}

impl SsvEventSyncer {
    #[instrument(skip(db, config), level = "debug")]
    /// Create a new SsvEventSyncer to sync all of the events from the chain
    pub async fn new(
        db: Arc<NetworkDatabase>,
        index_sync_tx: index_sync::Tx,
        exit_tx: ExitTx,
        slashing_protection: Arc<SlashingDatabase>,
        config: Config,
    ) -> Result<Self, ExecutionError> {
        info!("Creating new SSV Event Syncer");

        // Construct the rpc provider
        let rpc_client = http_with_timeout_and_fallback(&config.http_urls);
        debug!("Created rpc client");

        // Construct Websocket Provider
        let ws = WsConnect::new(config.ws_url.full.as_str());
        let ws_client = ProviderBuilder::default()
            .connect_ws(ws)
            .await
            .map_err(|e| {
                ExecutionError::SyncError(format!(
                    "Failed to bind to WS: {}, {}",
                    &config.ws_url, e
                ))
            })?;
        debug!("Created ws client");

        // Construct an EventProcessor with access to the DB
        let event_processor = EventProcessor::new(
            db.clone(),
            Mode::Node {
                index_sync_tx,
                exit_tx,
                slashing_protection,
            },
        );
        debug!("Created event processor - done");

        metrics::set_gauge(&metrics::EXECUTION_SYNC_STATUS, 0);

        Ok(Self {
            rpc_client,
            ws_client,
            ws_url: config.ws_url.full.into(),
            event_processor,
            network: config.network,
            is_synced: watch::channel(false).0,
        })
    }

    /// Create a new event syncer for a keysplit sync
    pub fn new_keysplit(
        db: Arc<NetworkDatabase>,
        rpc_endpoint: String,
        network: SsvNetworkConfig,
    ) -> Self {
        let http_url: Url = rpc_endpoint.parse().expect("Failed to parse HTTP URL");
        let rpc_client = ProviderBuilder::default().connect_http(http_url.clone());

        let event_processor = EventProcessor::new(db, Mode::KeySplit);

        // This does not perform a live sync, so we just want to mock websocket fields. This helps
        // so that we dont have to switch the ws fields to Option and clutter up the rest of the
        // application unnecessarily
        let ws_url = String::from("");
        let ws_client = ProviderBuilder::default().connect_http(http_url);

        Self {
            rpc_client,
            ws_client,
            ws_url,
            event_processor,
            network,
            is_synced: watch::channel(false).0,
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
                .historical_sync(contract_address, deployment_block, KEYSPLIT_EVENTS)
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
    pub fn is_synced(&self) -> watch::Receiver<bool> {
        self.is_synced.subscribe()
    }

    #[instrument(skip(self), level = "debug")]
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
                    self.is_synced.send_replace(false);

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
                }
            }
        }
    }

    // When we encounter a rpc error, keep polling until success
    async fn troubleshoot_rpc(&self) {
        info!("Attempting to reconnect to rpc");
        metrics::inc_counter_vec(&metrics::EXECUTION_CONNECTION_ERRORS, &["rpc"]);

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
        metrics::inc_counter_vec(&metrics::EXECUTION_CONNECTION_ERRORS, &["websocket"]);

        let mut retry_count = 0;
        let mut current_backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            let ws = WsConnect::new(&self.ws_url);
            if let Ok(ws_client) = ProviderBuilder::default().connect_ws(ws).await {
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
        metrics::inc_counter_vec(
            &metrics::EXECUTION_BACKOFF_ATTEMPTS,
            &[retry_count.to_string().as_str()],
        );

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

    #[instrument(skip(self), level = "debug")]
    /// Initial both a historical sync and a live sync from the chain. This function will transition
    /// into a never ending live sync, so it should never return
    pub async fn try_sync(
        &mut self,
        contract_address: Address,
        deployment_block: u64,
    ) -> Result<(), ExecutionError> {
        info!("Starting historical sync");
        self.historical_sync(contract_address, deployment_block, SSV_EVENTS)
            .await?;

        self.is_synced.send_replace(true);

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
    #[instrument(
        skip(self, contract_address, deployment_block, events),
        level = "debug"
    )]
    async fn historical_sync(
        &self,
        contract_address: Address,
        deployment_block: u64,
        events: &[&str],
    ) -> Result<(), ExecutionError> {
        // Start from the contract deployment block or the last block that has been processed
        let last_processed_block = self.event_processor.db.state().get_last_processed_block();
        let mut start_block = std::cmp::max(deployment_block, last_processed_block + 1);

        loop {
            let current_block = self.rpc_client.get_block_number().await.map_err(|e| {
                error!(?e, "Failed to fetch block number");
                ExecutionError::RpcError(format!("Failed to fetch block number: {e}"))
            })?;
            metrics::set_gauge(&metrics::EXECUTION_CURRENT_BLOCK, current_block as i64);

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

            struct Batch {
                logs: Vec<Log>,
                end_block: u64,
            }

            // Here, we have a start..end block that we need to sync the logs from. This range gets
            // broken up into individual ranges of BATCH_SIZE where the logs are fetches from. The
            // individual ranges are further broken up into a set of batches that are sequentually
            // processes. This makes it so we dont have a ton of logs that all have to be processed
            // in one pass

            // Chunk the start and end block range into a set of ranges of size BATCH_SIZE
            // and construct a future to fetch the logs in each range
            let mut pending_batches: VecDeque<_> = (start_block..=end_block)
                .step_by(BATCH_SIZE as usize)
                .map(|start| async move {
                    let (start, end) = (start, min(start + BATCH_SIZE - 1, end_block));
                    let logs = self
                        .fetch_logs(start, end, contract_address, events)
                        .await?;
                    Result::<Batch, ExecutionError>::Ok(Batch {
                        logs,
                        end_block: end,
                    })
                })
                .collect();

            let mut fetching_batches = FuturesOrdered::new();

            for _ in 0..FETCH_CONCURRENT {
                let Some(batch) = pending_batches.pop_front() else {
                    break;
                };
                fetching_batches.push_back(batch);
            }

            let mut fetched_batches = VecDeque::new();
            let mut running_processor = None;

            let mut batches_started = 0;

            info!(
                start_block = start_block,
                end_block = end_block,
                "Syncing all events"
            );
            loop {
                // Check if we should start processing a batch.
                let batch_to_run = if let Some(processor) = &mut running_processor {
                    // There is already a running batch processor, so let's wait until it finishes
                    // or another batch has been fetched.
                    select! {
                        Some(batch) = fetching_batches.next() => {
                            // A batch has been fetched, but a processor is running, so store the
                            // batch and do not start another batch yet.
                            fetched_batches.push_back(batch?);
                            None
                        }
                        result = processor => {
                            // Processor is done, so let's unregister it.
                            running_processor = None;
                            // Help rustc with type inference.
                            let result: Result<_, _> = result;
                            result.map_err(|e| ExecutionError::SyncError(format!("Event Processor Panicked: {e}")))??;
                            // Get the next batch that was fetched (if there is any)
                            fetched_batches.pop_front()
                        }
                    }
                } else {
                    // We have no running processor - this implies that `fetched_batches` is empty,
                    // as we start a batch immediately from there after a processor finishes.
                    // So we just have to wait for a batch from `fetching_batches`.
                    let Some(batch) = fetching_batches.next().await else {
                        // No running event processor and no more batches, we are done.
                        break;
                    };
                    Some(batch?)
                };

                if let Some(batch) = batch_to_run {
                    batches_started += 1;
                    if batches_started % LOG_AFTER_BATCHES == 0 {
                        info!(
                            processing_block = batch.end_block,
                            "Historical sync in progress"
                        )
                    }

                    let event_processor = self.event_processor.clone();
                    running_processor =
                        Some(spawn_blocking(move || -> Result<(), ExecutionError> {
                            event_processor.process_logs(batch.logs, false, batch.end_block)?;

                            metrics::set_gauge(
                                &metrics::EXECUTION_HISTORICAL_SYNC_PROGRESS,
                                batch.end_block as i64,
                            );

                            Ok(())
                        }));

                    if let Some(batch_to_fetch) = pending_batches.pop_front() {
                        // Start fetching another batch. We do this here (and not after a batch has
                        // been successfully fetched) to avoid downloading batches faster than we
                        // can process them.
                        fetching_batches.push_back(batch_to_fetch);
                    }
                }
            }

            info!("Processed all events up to block {}", end_block);

            // update end block processed information
            start_block = end_block + 1;
        }
        info!("Historical sync completed");
        Ok(())
    }

    // Construct a future that will fetch logs in the range from_block..to_block
    #[instrument(skip(self, deployment_address, events), level = "debug")]
    fn fetch_logs(
        &self,
        from_block: u64,
        to_block: u64,
        deployment_address: Address,
        events: &[&str],
    ) -> impl Future<Output = Result<Vec<Log>, ExecutionError>> + Send {
        // Setup filter and rpc client
        let rpc_client = self.rpc_client.clone();
        let filter = Filter::new()
            .address(deployment_address)
            .from_block(from_block)
            .to_block(to_block)
            .events(events);

        // Try to fetch logs with a retry upon error. Try up to MAX_RETRIES times and error if we
        // exceed this as we can assume there is some underlying connection issue
        async move {
            debug!("Fetching logs");
            let timer = metrics::start_timer_vec(
                &metrics::EXECUTION_LOG_FETCH_TIME,
                &[format!("{}", to_block - from_block + 1).as_str()],
            );

            match rpc_client.get_logs(&filter).await {
                Ok(logs) => {
                    debug!(log_count = logs.len(), "Successfully fetched logs");
                    metrics::stop_timer(timer);
                    Ok(logs)
                }
                Err(e) => {
                    // Subdivide if we have tried more than one block and if the error may be some
                    // kind of response size limit.
                    let subdivide = from_block != to_block
                        && matches!(
                            &e,
                            RpcError::Transport(TransportErrorKind::HttpError(_))
                                | RpcError::ErrorResp(_)
                        );

                    if subdivide {
                        self.subdivide_fetch_logs(
                            from_block,
                            to_block,
                            deployment_address,
                            events,
                            2,
                        )
                        .boxed()
                        .await
                    } else {
                        Err(ExecutionError::RpcError(format!(
                            "Error fetching logs: {e}"
                        )))
                    }
                }
            }
        }
    }

    // Subdivide log fetching to avoid log response size limits
    #[instrument(skip(self, deployment_address, events), level = "debug")]
    async fn subdivide_fetch_logs(
        &self,
        from_block: u64,
        to_block: u64,
        deployment_address: Address,
        events: &[&str],
        subdivision_factor: u64,
    ) -> Result<Vec<Log>, ExecutionError> {
        info!("Subdividing log retrieval");

        let num_blocks = (to_block - from_block) + 1;
        let target_size = max(1, num_blocks.div_ceil(subdivision_factor));
        let mut result = vec![];

        let mut current = from_block;
        while current <= to_block {
            let to = min(current + (target_size - 1), to_block);
            let logs = self
                .fetch_logs(current, to, deployment_address, events)
                .await?;
            result.extend(logs);
            current = to + 1;
        }

        Ok(result)
    }

    /// Exit logs need the block timestamps set. Ensure every exit in a batch of logs has a block
    /// timestamp set, fetching it from the EL if needed.
    async fn set_block_timestamps(&mut self, logs: &mut [Log]) -> Result<(), ExecutionError> {
        let mut block_timestamp_cache = HashMap::new();
        for log in logs.iter_mut() {
            if log.topic0() != Some(&SSVContract::ValidatorExited::SIGNATURE_HASH)
                || log.block_timestamp.is_some()
            {
                continue;
            }

            let block_number = log.block_number.ok_or_else(|| {
                ExecutionError::InvalidEvent("Block number not available".to_string())
            })?;

            if let Some(timestamp) = block_timestamp_cache.get(&block_number) {
                log.block_timestamp = Some(*timestamp);
            } else {
                trace!(block_number, "Block timestamp not available");

                let block = match self
                    .rpc_client
                    .get_block_by_number(BlockNumberOrTag::from(block_number))
                    .await
                {
                    Ok(Some(block)) => {
                        trace!(?block, "Fetched block");
                        block
                    }
                    Ok(None) => {
                        return Err(ExecutionError::InvalidEvent("Block not found".to_string()));
                    }
                    Err(e) => {
                        return Err(ExecutionError::RpcError(format!(
                            "Failed to fetch block {e}"
                        )));
                    }
                };

                // Store timestamp in log and cache in map
                log.block_timestamp = Some(block.header.timestamp);
                block_timestamp_cache.insert(block_number, block.header.timestamp);
            }
        }
        Ok(())
    }

    // Once caught up with the chain, start live sync which will stream in live blocks from the
    // network. The events will be processed and duties will be created in response to network
    // actions
    #[instrument(skip(self, contract_address), level = "debug")]
    async fn live_sync(&mut self, contract_address: Address) -> Result<(), ExecutionError> {
        info!(?contract_address, "Starting live sync");

        metrics::set_gauge(&metrics::EXECUTION_SYNC_STATUS, 1);

        loop {
            // Try to subscribe to a block stream
            let mut stream = self
                .ws_client
                .subscribe_blocks()
                .await
                .map_err(|e| {
                    ExecutionError::WsError(format!("Failed to subscribe to block stream: {e}"))
                })?
                .into_stream();

            info!("Successfully subscribed to block stream");

            // If we have a connection, continuously stream in blocks
            while let Some(block_header) = stream.next().await {
                // Block we are interested in is the current block number - follow distance
                let relevant_block = block_header.number - FOLLOW_DISTANCE;

                // If the relevant block was already processed, do not process it again. This can
                // happen if `block_header.number` was seen before due to a reorg.
                let last_processed_block =
                    self.event_processor.db.state().get_last_processed_block();
                if relevant_block <= last_processed_block {
                    debug!(
                        block_number = block_header.number,
                        relevant_block, "Already synced block - likely reorg"
                    );
                    continue;
                }

                debug!(
                    block_number = block_header.number,
                    relevant_block, "Processing new block"
                );

                metrics::set_gauge(
                    &metrics::EXECUTION_CURRENT_BLOCK,
                    block_header.number as i64,
                );

                let mut logs = self
                    .fetch_logs(
                        last_processed_block + 1,
                        relevant_block,
                        contract_address,
                        SSV_EVENTS,
                    )
                    .await?;

                self.set_block_timestamps(&mut logs).await?;

                let log_count = logs.len();

                self.event_processor
                    .process_logs(logs, true, relevant_block)?;

                info!(
                    log_count,
                    "Processed contract events from block {}", relevant_block
                );
            }

            // If we get here, the stream ended (likely due to disconnect)
            error!("WebSocket stream ended, reconnecting...");
            metrics::set_gauge(&metrics::EXECUTION_SYNC_STATUS, 0);
        }
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    use crate::util::http_with_timeout_and_fallback;

    #[tokio::test]
    async fn test_rpc_provider() {
        let urls = vec![
            SensitiveUrl::parse("https://eth.merkle.io").unwrap(),
            SensitiveUrl::parse("https://ethereum-rpc.publicnode.com").unwrap(),
        ];
        let provider = http_with_timeout_and_fallback(&urls);
        let block_number = provider.get_block_number().await;
        assert!(block_number.is_ok());
    }

    #[tokio::test]
    async fn test_rpc_provider_invalid_url() {
        let urls = vec![
            SensitiveUrl::parse("https://this-is-invalid.com").unwrap(),
            SensitiveUrl::parse("https://ethereum-rpc.publicnode.com").unwrap(),
        ];
        let provider = http_with_timeout_and_fallback(&urls);
        let block_number = provider.get_block_number().await;
        assert!(block_number.is_ok());
    }
}
