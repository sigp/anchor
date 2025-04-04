use std::{collections::HashMap, sync::Arc, time::Duration};

use beacon_node_fallback::BeaconNodeFallback;
use database::{ClusterMultiIndexMap, NetworkDatabase, UniqueIndex};
use eth2::types::{StateId, ValidatorId};
use slot_clock::SlotClock;
use ssv_types::{ValidatorIndex, ValidatorMetadata};
use task_executor::TaskExecutor;
use tokio::{
    select,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    time::sleep,
};
use tracing::{debug, error, info, warn};
use types::PublicKeyBytes;

pub type Tx = UnboundedSender<PublicKeyBytes>;

const INDEX_SYNCER_NAME: &str = "validator_index_syncer";

const MAX_BATCH_SIZE: usize = 512;
const BATCHING_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(45);

pub fn start_validator_index_syncer(
    nodes: Arc<BeaconNodeFallback<impl SlotClock + 'static>>,
    db: Arc<NetworkDatabase>,
    executor: TaskExecutor,
) -> Tx {
    let (tx, rx) = unbounded_channel();
    executor.spawn(validator_index_syncer(nodes, db, rx), INDEX_SYNCER_NAME);
    tx
}

async fn validator_index_syncer(
    nodes: Arc<BeaconNodeFallback<impl SlotClock>>,
    db: Arc<NetworkDatabase>,
    mut validator_queue_rx: UnboundedReceiver<PublicKeyBytes>,
) {
    info!("Starting validator index syncer");

    // counter to remember where we are in the sorted validator list
    // not perfect, as removed/added validators shift the list itself, but good enough for this
    let mut db_sweep = 0;

    loop {
        let mut batch = vec![];

        // first, take validators from the queue until the batch is full or there are no validators
        // for a bit
        while batch.len() < MAX_BATCH_SIZE {
            // wait at least MAX_DELAY if we got no incoming validators
            let max_delay = if batch.is_empty() {
                MAX_DELAY
            } else {
                BATCHING_DELAY
            };

            let space = MAX_BATCH_SIZE - batch.len();
            select! {
                got = validator_queue_rx.recv_many(&mut batch, space) => {
                    if got == 0 {
                        // queue is closed, we're probably shutting down
                        info!("Shutting down validator index syncer...");
                        return;
                    }
                }
                _ = sleep(max_delay) => {
                    debug!(?max_delay, "Time out waiting for validators");
                    break;
                }
            }
        }

        debug!(len = batch.len(), "Batched validators from queue");

        // next, fill up the rest of the batch with older validators that are unknown from the
        // database
        let space = MAX_BATCH_SIZE - batch.len();
        if space > 0 {
            let state = db.state();
            let clusters = state.clusters();
            let mut from_database = state
                .metadata()
                .values()
                .filter_map(|v| needs_index(v, &batch, clusters))
                .collect::<Vec<_>>();
            drop(state);
            let count = from_database.len();
            debug!(len = count, db_sweep, "Found unset index validators");

            // sort and skip to current position
            from_database.sort_unstable_by_key(|x| x.serialize());
            batch.extend(from_database.into_iter().skip(db_sweep).take(space));

            // update sweep, resetting it if necessary
            db_sweep += space;
            if db_sweep >= count {
                db_sweep = 0;
            }
        }

        if !batch.is_empty() {
            debug!(len = batch.len(), "Sending request");
            let validators = nodes
                .first_success(move |client| {
                    let batch = batch
                        .iter()
                        .copied()
                        .map(ValidatorId::PublicKey)
                        .collect::<Vec<_>>();
                    async move {
                        client
                            .post_beacon_states_validators(StateId::Head, Some(batch), None)
                            .await
                    }
                })
                .await
                .unwrap_or_else(|err| {
                    warn!(%err, "Failed to fetch validator indices");
                    None
                });

            let map = validators
                .into_iter()
                .flat_map(|v| v.data)
                .map(|v| (v.validator.pubkey, ValidatorIndex(v.index as usize)))
                .collect::<HashMap<_, _>>();
            debug!(len = map.len(), "Got validators from BN");
            if let Err(err) = db.set_validator_indices(map) {
                error!(?err, "Failed to update validator indices");
            }
        }
    }
}

fn needs_index(
    metadata: &ValidatorMetadata,
    current_batch: &[PublicKeyBytes],
    clusters: &ClusterMultiIndexMap,
) -> Option<PublicKeyBytes> {
    (metadata.index.is_none()
        && !current_batch.contains(&metadata.public_key)
        && clusters
            .get_by(&metadata.cluster_id)
            .is_some_and(|c| !c.liquidated))
    .then_some(metadata.public_key)
}
