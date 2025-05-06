use std::{sync::Arc, time::Duration};

use anchor_validator_store::AnchorValidatorStore;
use beacon_node_fallback::BeaconNodeFallback;
use slot_clock::SlotClock;
use ssv_types::ValidatorIndex;
use task_executor::TaskExecutor;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{error, info};
use types::{EthSpec, PublicKeyBytes, voluntary_exit};

// Message type for exit requests
pub struct ExitRequest {
    pub validator_pubkey: PublicKeyBytes,
    pub validator_index: ValidatorIndex,
    pub block_timestamp: u64,
}

pub type ExitTx = UnboundedSender<ExitRequest>;
pub type ExitRx = UnboundedReceiver<ExitRequest>;

const EXIT_PROCESSOR_NAME: &str = "voluntary_exit_processor";

pub fn start_exit_processor<E: EthSpec, T: SlotClock + 'static>(
    slot_clock: impl SlotClock + 'static,
    slots_per_epoch: u64,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    exit_tx: ExitRx,
    executor: TaskExecutor,
) {
    executor.spawn(
        process_exit_requests(
            slot_clock,
            slots_per_epoch,
            beacon_nodes,
            validator_store,
            exit_tx,
        ),
        EXIT_PROCESSOR_NAME,
    );
}

async fn process_exit_requests<E: EthSpec, T: SlotClock + 'static>(
    slot_clock: impl SlotClock + 'static,
    slots_per_epoch: u64,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    mut exit_rx: UnboundedReceiver<ExitRequest>,
) {
    info!("Starting voluntary exit processor");

    while let Some(request) = exit_rx.recv().await {
        process_exit_request(
            slot_clock.clone(),
            slots_per_epoch,
            beacon_nodes.clone(),
            validator_store.clone(),
            request,
        )
        .await;
    }

    info!("Exit processor shutting down");
}

async fn process_exit_request<E: EthSpec, T: SlotClock + 'static>(
    slot_clock: impl SlotClock + 'static,
    slots_per_epoch: u64,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    request: ExitRequest,
) {
    let ExitRequest {
        validator_pubkey,
        validator_index,
        block_timestamp,
    } = request;

    info!(
        validator_pubkey = %validator_pubkey,
        validator_index = ?validator_index,
        "Processing voluntary exit request"
    );

    let block_time = Duration::from_millis(block_timestamp);
    const VOLUNTARY_EXIT_SLOTS_TO_POSTPONE: u64 = 4;
    let slot =
        slot_clock.slot_of(block_time).unwrap_or_default() + VOLUNTARY_EXIT_SLOTS_TO_POSTPONE;

    let epoch = slot.epoch(slots_per_epoch);

    let voluntary_exit = voluntary_exit::VoluntaryExit {
        epoch,
        validator_index: validator_index.0 as u64,
    };

    match validator_store
        .collect_voluntary_exit_signatures(validator_pubkey, voluntary_exit, slot)
        .await
    {
        Ok(signed_exit) => {
            // Submit to beacon node
            match beacon_nodes
                .first_success(|client| {
                    let signed_voluntary_exit = signed_exit.clone();
                    async move {
                        client
                            .post_beacon_pool_voluntary_exits(&signed_voluntary_exit)
                            .await
                    }
                })
                .await
            {
                Ok(_) => {
                    info!(
                        validator_pubkey = %validator_pubkey,
                        "Successfully submitted voluntary exit to beacon node"
                    );
                    metrics::inc_counter_vec(&crate::metrics::EXECUTION_EVENTS_PROCESSED, &[
                        "validator_exited",
                    ]);
                }
                Err(e) => {
                    error!(
                        validator_pubkey = %validator_pubkey,
                        error = %e,
                        "Failed to submit voluntary exit to beacon node"
                    );
                }
            }
        }
        Err(e) => {
            error!(
                validator_pubkey = %validator_pubkey,
                error = ?e,
                "Failed to collect signatures for validator exit"
            );
        }
    }
}
