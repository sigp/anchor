use beacon_node_fallback::BeaconNodeFallback;
use database::NetworkDatabase;
use eth2::types::{StateId, ValidatorId};
use rand::rng;
use rand::seq::SliceRandom;
use slot_clock::SlotClock;
use ssv_types::ValidatorIndex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use task_executor::TaskExecutor;
use tokio::select;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use types::{EthSpec, PublicKeyBytes};

pub type Tx = UnboundedSender<PublicKeyBytes>;

const INDEX_SYNCER_NAME: &str = "validator_index_syncer";

const MAX_BATCH_SIZE: usize = 512;
const BATCHING_DELAY: Duration = Duration::from_secs(1);

pub fn start_validator_index_syncer<E: EthSpec>(
    nodes: Arc<BeaconNodeFallback<impl SlotClock + 'static>>,
    db: Arc<NetworkDatabase>,
    slot_clock: impl SlotClock + 'static,
    executor: TaskExecutor,
) -> Tx {
    let (tx, rx) = unbounded_channel();
    executor.spawn(
        validator_index_syncer::<E>(nodes, db, slot_clock, rx),
        INDEX_SYNCER_NAME,
    );
    tx
}

async fn validator_index_syncer<E: EthSpec>(
    nodes: Arc<BeaconNodeFallback<impl SlotClock>>,
    db: Arc<NetworkDatabase>,
    slot_clock: impl SlotClock,
    mut validator_queue_rx: UnboundedReceiver<PublicKeyBytes>,
) {
    info!("Starting validator index syncer");
    loop {
        let mut batch = vec![];

        // first, take validators from the queue until the batch is full or there are no validators
        // for a bit
        while batch.len() < MAX_BATCH_SIZE {
            // if the batch is empty, wait up until next epoch - because we want to retry from the
            // database then. If batch is not empty, do not wait too long - we want to query those
            // ASAP
            let max_delay = if batch.is_empty() {
                slot_clock
                    .duration_to_next_epoch(E::slots_per_epoch())
                    .unwrap_or(BATCHING_DELAY)
            } else {
                BATCHING_DELAY
            };

            select! {
                item = validator_queue_rx.recv() => {
                    if let Some(item) = item {
                        batch.push(ValidatorId::PublicKey(item));
                    } else {
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
            let mut from_database = db
                .state()
                .metadata()
                .values()
                .filter_map(|v| {
                    let public_key = ValidatorId::PublicKey(v.public_key);
                    (v.index.is_none() && !batch.contains(&public_key)).then_some(public_key)
                })
                .collect::<Vec<_>>();
            debug!(len = from_database.len(), "Found unset index validators");
            from_database.shuffle(&mut rng());
            batch.extend(from_database.into_iter().take(space));
        }

        if !batch.is_empty() {
            let validators = match nodes
                .first_success(move |client| {
                    let batch = batch.clone();
                    async move {
                        client
                            .get_beacon_states_validators(StateId::Head, Some(&batch), None)
                            .await
                    }
                })
                .await
            {
                Ok(validators) => validators,
                Err(err) => {
                    warn!(%err, "Failed to fetch validator indices");
                    return;
                }
            };

            let map = validators
                .into_iter()
                .flat_map(|v| v.data)
                .map(|v| (v.validator.pubkey, ValidatorIndex(v.index as usize)))
                .collect::<HashMap<_, _>>();
            if let Err(err) = db.set_validator_indices(map) {
                error!(?err, "Failed to update validator indices");
            }
        }
    }
}
