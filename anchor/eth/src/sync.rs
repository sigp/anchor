use crate::gen::SSVContract;
use alloy::primitives::FixedBytes;
use alloy::primitives::{address, Address};
use alloy::providers::{Provider, ProviderBuilder, RootProvider, WsConnect};
use alloy::pubsub::PubSubFrontend;
use alloy::rpc::types::Filter;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy::transports::http::{Client, Http};
use futures::future::join_all;
use rand::Rng;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::time::Duration;

/// SSV contract events needed to come up to date with the network
static SSV_EVENTS: LazyLock<Vec<FixedBytes<32>>> = LazyLock::new(|| {
    vec![
        // event OperatorAdded(uint64 indexed operatorId, address indexed owner, bytes publicKey, uint256 fee);
        SSVContract::OperatorAdded::SIGNATURE_HASH,
        // event OperatorRemoved(uint64 indexed operatorId);
        SSVContract::OperatorRemoved::SIGNATURE_HASH,
        // event ValidatorAdded(address indexed owner, uint64[] operatorIds, bytes publicKey, bytes shares, Cluster cluster);
        SSVContract::ValidatorAdded::SIGNATURE_HASH,
        // event ValidatorRemoved(address indexed owner, uint64[] operatorIds, bytes publicKey, Cluster cluster);
        SSVContract::ValidatorRemoved::SIGNATURE_HASH,
        // event ClusterLiquidated(address indexed owner, uint64[] operatorIds, Cluster cluster);
        SSVContract::ClusterLiquidated::SIGNATURE_HASH,
        // event ClusterReactivated(address indexed owner, uint64[] operatorIds, Cluster cluster);
        SSVContract::ClusterReactivated::SIGNATURE_HASH,
        // event FeeRecipientAddressUpdated(address indexed owner, address recipientAddress);
        SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH,
        // event ValidatorExited(address indexed owner, uint64[] operatorIds, bytes publicKey);
        SSVContract::ValidatorExited::SIGNATURE_HASH,
    ]
});

/// Contract deployment address
/// https://etherscan.io/address/0xDD9BC35aE942eF0cFa76930954a156B3fF30a4E1
static CONTRACT_DEPLOYMENT_ADDRESS: LazyLock<Address> =
    LazyLock::new(|| address!("DD9BC35aE942eF0cFa76930954a156B3fF30a4E1"));

/// Contract deployment block on Ethereum Mainnet
/// https://etherscan.io/tx/0x4a11a560d3c2f693e96f98abb1feb447646b01b36203ecab0a96a1cf45fd650b
const CONTRACT_DEPLOYMENT_BLOCK: u64 = 17507487;

/// Batch size for log fetching
/// todo!(), play around with this number, default max logs per filter is 20k and this contract is
/// not event heavy, so I think this could be increased a lot
const BATCH_SIZE: u64 = 500;

/// Typedef RPC and WS clients
type RpcClient = RootProvider<Http<Client>>;
type WsClient = RootProvider<PubSubFrontend>;

// Retry information for log fetching
// todo!() backoff if needed
const MAX_RETRIES: i32 = 5;

/// Client for interacting with the SSV contract on Ethereum L1
///
/// Manages connections to the L1 and monitors SSV contract events to track the state of validator
/// and operators. Provides both historical synchronization and live event monitoring
pub struct SsvEventSyncer {
    /// Http client connected to the L1 to fetch historical SSV event information
    rpc_client: Arc<RpcClient>,
    // Websocket client connected to L1 to stream live SSV event information
    ws_client: WsClient,
}

impl SsvEventSyncer {
    pub async fn new() -> Result<Self, String> {
        // Construct HTTP Provider
        let http_url = "dummy_http".parse().unwrap(); // TODO!(), get this from config
        let rpc_client: Arc<RpcClient> = Arc::new(ProviderBuilder::new().on_http(http_url));

        // Construct Websocket Provider
        let ws_url = "dummy ws"; // TODO!(), get this from config
        let ws_client = ProviderBuilder::new()
            .on_ws(WsConnect::new(ws_url))
            .await
            .map_err(|e| format!("Failed to bind to WS: {}, {}", ws_url, e))?;

        Ok(Self {
            rpc_client,
            ws_client,
        })
    }

    // Top level function to sync data
    pub async fn sync(&self) -> Result<(), String> {
        // first, perform a historical sync
        self.historical_sync().await?;

        // todo!() blocks are still added while we are syncing historical state
        // impl to catch up to head - follow distance

        // once the historical sync is done and we have processed them, start a live sync
        // todo!(), live sync

        // OK. We have done the historical sync and spawned off live sync to process
        Ok(())
    }

    /// Perform a historical sync from the contract deployment block to catch up to the current
    /// state of the SSV network
    async fn historical_sync(&self) -> Result<(), String> {
        // todo!() impl follow distance
        // Fetch range from start_block..(current_block-follow_distance)
        let start_block = CONTRACT_DEPLOYMENT_BLOCK;
        let current_block = self.rpc_client.get_block_number().await.unwrap();

        // Chunk the start and end block range into a set of ranges of size BATCH_SIZE and construct
        // a new task to fetch the logs from each range
        let tasks: Vec<_> = (start_block..=current_block)
            .step_by(BATCH_SIZE as usize)
            .map(|start| {
                let (start, end) = (start, std::cmp::min(start + BATCH_SIZE - 1, current_block));
                self.fetch_logs(start, end)
            })
            .collect();

        // Await all of the futures. This will panic if one of the futures is unsuccessful.
        let event_logs: Vec<Log> = join_all(tasks).await.into_iter().flatten().collect();

        // The futures may join out of order block wise. The individual events within the block
        // retain their tx ordering. Due to this, we can reassemble back into blocks and be
        // confident the order is correct
        let mut ordered_event_logs: BTreeMap<u64, Vec<Log>> = BTreeMap::new();
        for log in event_logs {
            let block_num = log.block_number.expect("Log should have a block number");
            ordered_event_logs.entry(block_num).or_default().push(log);
        }

        // join them back to a vec in ordered format
        let ordered_event_logs: Vec<Log> = ordered_event_logs.into_values().flatten().collect();

        // Logs are all fetched from the chain and in order, process them
        //self.event_processor.process_logs(ordered_event_logs)?;
        Ok(())
    }

    /// Fetch logs from the chain
    fn fetch_logs(&self, from_block: u64, to_block: u64) -> impl Future<Output = Vec<Log>> {
        // Setup filter and rpc client
        let rpc_client = self.rpc_client.clone();
        let filter = Filter::new()
            .address(*CONTRACT_DEPLOYMENT_ADDRESS)
            .from_block(from_block)
            .to_block(to_block)
            .events(&*SSV_EVENTS);

        // Try to fetch logs with a retry upon error. Try up to MAX_RETRIES times and error if we
        // exceed this as we can assume there is some underlying connection issue
        async move {
            let mut retry_cnt = 0;
            loop {
                match rpc_client.get_logs(&filter).await {
                    Ok(logs) => return logs,
                    Err(_) => {
                        // confirm we have not exceeded max
                        if retry_cnt > MAX_RETRIES {
                            panic!("Unable to fetch logs");
                        }

                        // increment retry_count and jitter retry duration
                        // todo!() exponential backoff??
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

    /// Live sync with the chain to get new contract events while enforcing a follow distance
    fn live_sync(&self) {
        // Do we want to stream new blocks in via a websocket connection or poll at regular
        // intervals for new blocks?
        //
        //
        // Get new events & process them. Will maintain very similar flow to historical sync except
        // we want to set some flag signaling to handler that we want to forward these notifications
        // to the central processor for some action to be taken
        todo!()
    }
}
