use crate::error::ExecutionError;
use crate::event_processor::EventProcessor;
use crate::gen::SSVContract;
use alloy::primitives::{address, Address};
use alloy::providers::{Provider, ProviderBuilder, RootProvider, WsConnect};
use alloy::pubsub::PubSubFrontend;
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use alloy::transports::http::{Client, Http};
use database::NetworkDatabase;
use futures::future::{try_join_all, Future};
use futures::StreamExt;
use rand::Rng;
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use tokio::time::Duration;
use tracing::{debug, error, info, instrument, warn};

/// SSV contract events needed to come up to date with the network
static SSV_EVENTS: LazyLock<Vec<&str>> = LazyLock::new(|| {
    vec![
        // event OperatorAdded(uint64 indexed operatorId, address indexed owner, bytes publicKey, uint256 fee);
        SSVContract::OperatorAdded::SIGNATURE,
        // event OperatorRemoved(uint64 indexed operatorId);
        SSVContract::OperatorRemoved::SIGNATURE,
        // event ValidatorAdded(address indexed owner, uint64[] operatorIds, bytes publicKey, bytes shares, Cluster cluster);
        SSVContract::ValidatorAdded::SIGNATURE,
        // event ValidatorRemoved(address indexed owner, uint64[] operatorIds, bytes publicKey, Cluster cluster);
        SSVContract::ValidatorRemoved::SIGNATURE,
        // event ClusterLiquidated(address indexed owner, uint64[] operatorIds, Cluster cluster);
        SSVContract::ClusterLiquidated::SIGNATURE,
        // event ClusterReactivated(address indexed owner, uint64[] operatorIds, Cluster cluster);
        SSVContract::ClusterReactivated::SIGNATURE,
        // event FeeRecipientAddressUpdated(address indexed owner, address recipientAddress);
        SSVContract::FeeRecipientAddressUpdated::SIGNATURE,
        // event ValidatorExited(address indexed owner, uint64[] operatorIds, bytes publicKey);
        SSVContract::ValidatorExited::SIGNATURE,
    ]
});

/// Contract deployment addresses
/// Mainnet: https://etherscan.io/address/0xDD9BC35aE942eF0cFa76930954a156B3fF30a4E1
static MAINNET_DEPLOYMENT_ADDRESS: LazyLock<Address> =
    LazyLock::new(|| address!("DD9BC35aE942eF0cFa76930954a156B3fF30a4E1"));

/// Holesky: https://holesky.etherscan.io/address/0x38A4794cCEd47d3baf7370CcC43B560D3a1beEFA
static HOLESKY_DEPLOYMENT_ADDRESS: LazyLock<Address> =
    LazyLock::new(|| address!("38A4794cCEd47d3baf7370CcC43B560D3a1beEFA"));

/// Contract deployment block on Ethereum Mainnet
/// Mainnet: https://etherscan.io/tx/0x4a11a560d3c2f693e96f98abb1feb447646b01b36203ecab0a96a1cf45fd650b
const MAINNET_DEPLOYMENT_BLOCK: u64 = 17507487;

/// Contract deployment block on the Holesky Network
/// Holesky: https://holesky.etherscan.io/tx/0x998c38ff37b47e69e23c21a8079168b7e0e0ade7244781587b00be3f08a725c6
const HOLESKY_DEPLOYMENT_BLOCK: u64 = 181612;

/// Batch size for log fetching
const BATCH_SIZE: u64 = 10000;

/// Batch size for task groups
const GROUP_SIZE: usize = 50;

/// RPC and WS clients types
type RpcClient = RootProvider<Http<Client>>;
type WsClient = RootProvider<PubSubFrontend>;

/// Retry information for log fetching
const MAX_RETRIES: i32 = 5;

// Block follow distance
const FOLLOW_DISTANCE: u64 = 8;

/// The maximum number of operators a validator can have
/// https://github.com/ssvlabs/ssv/blob/07095fe31e3ded288af722a9c521117980585d95/eth/eventhandler/validation.go#L15
pub const MAX_OPERATORS: usize = 13;

/// Network that is being connected to
#[derive(Debug)]
pub enum Network {
    Mainnet,
    Holesky,
}

// TODO!() Dummy config struct that will be replaced
#[derive(Debug)]
pub struct Config {
    pub http_url: String,
    pub ws_url: String,
    pub beacon_url: String,
    pub network: Network,
}

/// Client for interacting with the SSV contract on Ethereum L1
///
/// Manages connections to the L1 and monitors SSV contract events to track the state of validator
/// and operators. Provides both historical synchronization and live event monitoring
pub struct SsvEventSyncer {
    /// Http client connected to the L1 to fetch historical SSV event information
    rpc_client: Arc<RpcClient>,
    /// Websocket client connected to L1 to stream live SSV event information
    ws_client: WsClient,
    /// Websocket connection url
    ws_url: String,
    /// Event processor for logs
    event_processor: EventProcessor,
    /// The network the node is connected to
    network: Network,
}

impl SsvEventSyncer {
    #[instrument(skip(db))]
    /// Create a new SsvEventSyncer to sync all of the events from the chain
    pub async fn new(db: Arc<NetworkDatabase>, config: Config) -> Result<Self, ExecutionError> {
        info!(?config, "Creating new SSV Event Syncer");

        // Construct HTTP Provider
        let http_url = config.http_url.parse().expect("Failed to parse HTTP URL");
        let rpc_client: Arc<RpcClient> = Arc::new(ProviderBuilder::new().on_http(http_url));

        // Construct Websocket Provider
        let ws = WsConnect::new(&config.ws_url);
        let ws_client = ProviderBuilder::new()
            .on_ws(ws.clone())
            .await
            .map_err(|e| {
                ExecutionError::SyncError(format!(
                    "Failed to bind to WS: {}, {}",
                    &config.ws_url, e
                ))
            })?;

        // Construct an EventProcessor with access to the DB
        let event_processor = EventProcessor::new(db);

        Ok(Self {
            rpc_client,
            ws_client,
            ws_url: config.ws_url,
            event_processor,
            network: config.network,
        })
    }

    #[instrument(skip(self))]
    /// Initial both a historical sync and a live sync from the chain. This function will transition
    /// into a never ending live sync, so it should never return
    pub async fn sync(&mut self) -> Result<(), ExecutionError> {
        info!("Starting SSV event sync");

        // Get network specific contract information
        let (contract_address, deployment_block) = match self.network {
            Network::Mainnet => (*MAINNET_DEPLOYMENT_ADDRESS, MAINNET_DEPLOYMENT_BLOCK),
            Network::Holesky => (*HOLESKY_DEPLOYMENT_ADDRESS, HOLESKY_DEPLOYMENT_BLOCK),
        };

        info!(
            ?contract_address,
            deployment_block, "Using contract configuration"
        );

        info!("Starting historical sync");
        self.historical_sync(contract_address, deployment_block)
            .await?;

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
    #[instrument(skip(self, contract_address, deployment_block))]
    async fn historical_sync(
        &self,
        contract_address: Address,
        deployment_block: u64,
    ) -> Result<(), ExecutionError> {
        // Start from the contract deployment block or the last block that has been processed
        let last_processed_block = self.event_processor.db.get_last_processed_block() + 1;
        let mut start_block = std::cmp::max(deployment_block, last_processed_block);

        loop {
            let current_block = self.rpc_client.get_block_number().await.map_err(|e| {
                error!(?e, "Failed to fetch block number");
                ExecutionError::RpcError(format!("Unable to fetch block number {}", e))
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
            if start_block == end_block {
                info!("Synced up to the tip of the chain, breaking");
                break;
            }

            // Here, we have a start..endblock that we need to sync the logs from. This range gets
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
                    self.fetch_logs(start, end, contract_address)
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
                    ExecutionError::SyncError(format!("Failed to join log future: {e}"))
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
    #[instrument(skip(self, deployment_address))]
    fn fetch_logs(
        &self,
        from_block: u64,
        to_block: u64,
        deployment_address: Address,
    ) -> impl Future<Output = Result<Vec<Log>, ExecutionError>> {
        // Setup filter and rpc client
        let rpc_client = self.rpc_client.clone();
        let filter = Filter::new()
            .address(deployment_address)
            .from_block(from_block)
            .to_block(to_block)
            .events(&*SSV_EVENTS);

        // Try to fetch logs with a retry upon error. Try up to MAX_RETRIES times and error if we
        // exceed this as we can assume there is some underlying connection issue
        async move {
            let mut retry_cnt = 0;
            loop {
                match rpc_client.get_logs(&filter).await {
                    Ok(logs) => {
                        debug!(log_count = logs.len(), "Successfully fetched logs");
                        return Ok(logs);
                    }
                    Err(e) => {
                        if retry_cnt > MAX_RETRIES {
                            error!(?e, retry_cnt, "Max retries exceeded while fetching logs");
                            return Err(ExecutionError::RpcError(
                                "Unable to fetch logs".to_string(),
                            ));
                        }

                        warn!(?e, retry_cnt, "Error fetching logs, retrying");

                        // increment retry_count and jitter retry duration
                        let jitter = rand::thread_rng().gen_range(0..=100);
                        let sleep_duration = Duration::from_millis(jitter);
                        tokio::time::sleep(sleep_duration).await;
                        retry_cnt += 1;
                        continue;
                    }
                }
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
                    error!(
                        ?e,
                        "Failed to subscribe to block stream. Retrying in 1 second..."
                    );

                    // Backend has closed, need to reconnect
                    let ws = WsConnect::new(&self.ws_url);
                    if let Ok(ws_client) = ProviderBuilder::new().on_ws(ws).await {
                        info!("Successfully reconnected to websocket. Catching back up");
                        self.ws_client = ws_client;
                        // Historical sync any missed blocks while down, can pass 0 as deployment
                        // block since it will use last_processed_block from DB anyways
                        self.historical_sync(contract_address, 0).await?;
                    } else {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    None
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
                        .fetch_logs(relevant_block, relevant_block, contract_address)
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
