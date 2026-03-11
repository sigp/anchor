use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use beacon_node_fallback::BeaconNodeFallback;
use bls::PublicKeyBytes;
use database::{ClusterMultiIndexMap, NetworkDatabase, UniqueIndex};
use eth2::types::{StateId, ValidatorId};
use slot_clock::SlotClock;
use ssv_types::{ValidatorIndex, ValidatorMetadata};
use task_executor::TaskExecutor;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    time::sleep,
};
use tracing::{debug, error, info, trace, warn};

pub type Tx = UnboundedSender<PublicKeyBytes>;

const INDEX_SYNCER_NAME: &str = "validator_index_syncer";
const INDEX_SYNCER_STORE_NAME: &str = "validator_index_syncer_store";

const MAX_BATCH_SIZE: usize = 512;
const BATCHING_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(45);

pub fn start_validator_index_syncer(
    nodes: Arc<BeaconNodeFallback<impl SlotClock + 'static>>,
    db: Arc<NetworkDatabase>,
    executor: TaskExecutor,
) -> Tx {
    let (tx, rx) = unbounded_channel();
    executor.spawn(
        validator_index_syncer(nodes, db, rx, executor.clone()),
        INDEX_SYNCER_NAME,
    );
    tx
}

async fn validator_index_syncer(
    nodes: Arc<BeaconNodeFallback<impl SlotClock>>,
    db: Arc<NetworkDatabase>,
    mut validator_queue_rx: UnboundedReceiver<PublicKeyBytes>,
    executor: TaskExecutor,
) {
    info!("Starting validator index syncer");

    // counter to remember where we are in the sorted validator list
    // not perfect, as removed/added validators shift the list itself, but good enough for this
    let mut missing_index_scan_cursor = 0;

    // Track if there are store tasks waiting. If there are any waiting tasks, we do not fill up
    // batches from the database to avoid redundant work.
    let pending_store_writes = Arc::new(AtomicUsize::new(0));

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

        trace!(len = batch.len(), "Batched validators from queue");

        // next, fill up the rest of the batch with older validators that are unknown from the
        // database
        fill_batch_with_missing_indices_from_db(
            &mut batch,
            &db,
            &pending_store_writes,
            &mut missing_index_scan_cursor,
        );

        if !batch.is_empty() {
            trace!(len = batch.len(), "Sending request");
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
            trace!(len = map.len(), "Got validators from BN");

            // `set_validator_indices` may block as it starts a database transaction and updates the
            // in memory database. We do not want to do that on the async runtime, so we
            // spawn a blocking task.
            let db = db.clone();
            let pending_store_writes = pending_store_writes.clone();
            pending_store_writes.fetch_add(1, Ordering::Relaxed);
            executor.spawn_blocking(
                move || {
                    let len = map.len();
                    if let Err(err) = db.set_validator_indices(map) {
                        error!(?err, "Failed to update validator indices");
                    } else {
                        trace!(len, "Stored indices from BN");
                    }
                    pending_store_writes.fetch_sub(1, Ordering::Relaxed);
                },
                INDEX_SYNCER_STORE_NAME,
            );
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

/// If there is space left in the batch, look up validators from the database that are missing
/// indices. This is skipped if there are any pending store writes, as these store writes might
/// add missing indices, and we want to avoid double lookups.
fn fill_batch_with_missing_indices_from_db(
    batch: &mut Vec<PublicKeyBytes>,
    db: &NetworkDatabase,
    pending_store_writes: &AtomicUsize,
    missing_index_scan_cursor: &mut usize,
) {
    let space = MAX_BATCH_SIZE - batch.len();
    // Only do this if we have any space remaining and there are no store tasks that might wait
    // to write missing indices. If the count is 1, only we hold the Arc (no other tasks).
    // We do this to avoid DB candidates while writes are active.
    if space > 0 && pending_store_writes.load(Ordering::Relaxed) == 0 {
        let state = db.state();
        let clusters = state.clusters();
        let mut from_database = state
            .metadata()
            .values()
            .filter_map(|v| needs_index(v, batch, clusters))
            .collect::<Vec<_>>();
        drop(state);
        let count = from_database.len();
        debug!(
            len = count,
            missing_index_scan_cursor, "Found unset index validators"
        );

        // sort and skip to current position
        from_database.sort_unstable_by_key(|x| x.serialize());
        batch.extend(
            from_database
                .into_iter()
                .skip(*missing_index_scan_cursor)
                .take(space),
        );

        // update sweep, resetting it if necessary
        *missing_index_scan_cursor += space;
        if *missing_index_scan_cursor >= count {
            *missing_index_scan_cursor = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use database::test_utils::InMemoryTestFixture;

    use super::*;

    #[test]
    fn do_not_fill_up_batch_if_store_tasks_are_waiting() {
        let mut batch = vec![];
        let fixture = InMemoryTestFixture::new();

        fill_batch_with_missing_indices_from_db(
            &mut batch,
            &fixture.db,
            &AtomicUsize::new(1),
            &mut 0,
        );

        assert_eq!(batch, vec![]);
    }
}
