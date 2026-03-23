use std::{collections::HashMap, sync::Arc, time::Duration};

use beacon_node_fallback::BeaconNodeFallback;
use bls::PublicKeyBytes;
use database::{ClusterMultiIndexMap, DatabaseError, NetworkDatabase, UniqueIndex};
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

#[derive(Debug)]
enum StoreValidatorIndicesError {
    RuntimeShuttingDown,
    Join(tokio::task::JoinError),
    Database(DatabaseError),
}

pub fn start_validator_index_syncer(
    nodes: Arc<BeaconNodeFallback<impl SlotClock + 'static>>,
    db: Arc<NetworkDatabase>,
    executor: TaskExecutor,
) -> Tx {
    let (tx, rx) = unbounded_channel();
    let sync_executor = executor.clone();
    executor.spawn(
        validator_index_syncer(nodes, db, rx, sync_executor),
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

        trace!(len = batch.len(), "Batched validators from queue");

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
            let len = map.len();
            trace!(len, "Got validators from BN");
            match store_validator_indices_blocking(&executor, Arc::clone(&db), map).await {
                Ok(()) => trace!(len, "Stored indices from BN"),
                Err(StoreValidatorIndicesError::RuntimeShuttingDown) => {
                    error!(
                        "Failed to spawn blocking validator index store task: runtime shutting down"
                    );
                    return;
                }
                Err(StoreValidatorIndicesError::Join(err)) => {
                    error!(?err, "Blocking validator index store task failed");
                }
                Err(StoreValidatorIndicesError::Database(err)) => {
                    error!(?err, "Failed to update validator indices");
                }
            }
        }
    }
}

async fn store_validator_indices_blocking(
    executor: &TaskExecutor,
    db: Arc<NetworkDatabase>,
    map: HashMap<PublicKeyBytes, ValidatorIndex>,
) -> Result<(), StoreValidatorIndicesError> {
    let Some(store_task) = executor.spawn_blocking_handle(
        move || db.set_validator_indices(map),
        INDEX_SYNCER_STORE_NAME,
    ) else {
        return Err(StoreValidatorIndicesError::RuntimeShuttingDown);
    };

    let store_result = store_task.await.map_err(StoreValidatorIndicesError::Join)?;
    store_result.map_err(StoreValidatorIndicesError::Database)
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

#[cfg(test)]
mod tests {
    use database::test_utils::{InMemoryTestFixture, queries};
    use task_executor::test_utils::TestRuntime;

    use super::*;

    const UPDATED_VALIDATOR_INDEX: usize = 777;

    fn create_test_executor() -> TaskExecutor {
        let test_runtime = TestRuntime::default();
        test_runtime.task_executor.clone()
    }

    // ==================== Blocking store tests ====================

    /// Ensures validator index writes run on the blocking pool while still completing before the
    /// sync loop continues.
    #[tokio::test]
    async fn test_store_validator_indices_blocking_updates_database_and_state() {
        // Arrange: create a populated DB with a known validator and a test executor.
        let fixture = InMemoryTestFixture::new();
        let validator_pubkey = fixture.validator.public_key;
        let db = Arc::new(fixture.data.db);
        let executor = create_test_executor();
        let updated_index = ValidatorIndex(UPDATED_VALIDATOR_INDEX);
        let index_updates = HashMap::from([(validator_pubkey, updated_index)]);

        // Act: offload the write to the blocking pool and await its completion.
        let result =
            store_validator_indices_blocking(&executor, Arc::clone(&db), index_updates).await;

        // Assert: both the durable DB row and the in-memory state reflect the new index.
        assert!(
            result.is_ok(),
            "blocking validator-index store should complete successfully"
        );

        let state = db.state();
        let stored_state_index = state
            .metadata()
            .get_by(&validator_pubkey)
            .and_then(|metadata| metadata.index);
        drop(state);
        assert_eq!(stored_state_index, Some(updated_index));

        let mut conn = db
            .connection()
            .expect("test should get a database connection");
        let tx = conn.transaction().expect("test should open a transaction");
        let stored_validator = queries::get_validator(&validator_pubkey.to_string(), &tx)
            .expect("validator should remain present in the database");
        assert_eq!(stored_validator.index, Some(updated_index));
    }
}
