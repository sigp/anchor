use std::{sync::Arc, time::Duration};

use anchor_validator_store::AnchorValidatorStore;
use beacon_node_fallback::BeaconNodeFallback;
use slot_clock::SlotClock;
use ssv_types::ValidatorIndex;
use task_executor::TaskExecutor;
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    time::sleep,
};
use tracing::{debug, error, info};
use types::{EthSpec, PublicKeyBytes, voluntary_exit};

use crate::voluntary_exit_tracker::{ExitDuty, VoluntaryExitTracker};

// Message type for exit requests
pub struct ExitRequest {
    pub validator_pubkey: PublicKeyBytes,
    pub validator_index: ValidatorIndex,
    pub block_timestamp: u64,
    pub is_our_validator: bool,
}

pub type ExitTx = UnboundedSender<ExitRequest>;
pub type ExitRx = UnboundedReceiver<ExitRequest>;

const EXIT_RECEIVER_NAME: &str = "voluntary_exit_receiver";
const EXIT_PROCESSOR_NAME: &str = "voluntary_exit_processor";
const VOLUNTARY_EXIT_SLOTS_TO_POSTPONE: u64 = 4;

pub fn start_exit_processor<E: EthSpec, T: SlotClock + 'static>(
    slot_clock: impl SlotClock + 'static,
    slots_per_epoch: u64,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    exit_rx: ExitRx,
    executor: TaskExecutor,
    exit_tracker: Arc<VoluntaryExitTracker>,
) {
    // This processor handles receiving exit requests and scheduling them
    let tracker_clone = exit_tracker.clone();
    let slot_clock_clone = slot_clock.clone();
    executor.spawn(
        async move {
            receive_exit_requests(slot_clock_clone, tracker_clone, exit_rx).await;
        },
        EXIT_RECEIVER_NAME,
    );

    // This processor handles executing the scheduled exits at the appropriate slots
    executor.spawn(
        async move {
            process_scheduled_exits(
                slot_clock,
                slots_per_epoch,
                beacon_nodes,
                validator_store,
                exit_tracker,
            )
            .await;
        },
        EXIT_PROCESSOR_NAME,
    );
}

/// Receives exit requests and schedules them for later processing
async fn receive_exit_requests(
    slot_clock: impl SlotClock + 'static,
    exit_tracker: Arc<VoluntaryExitTracker>,
    mut exit_rx: UnboundedReceiver<ExitRequest>,
) {
    info!("Starting voluntary exit request receiver");

    while let Some(request) = exit_rx.recv().await {
        let ExitRequest {
            validator_pubkey,
            validator_index,
            block_timestamp,
            is_our_validator,
        } = request;

        let exit_slot = slot_clock
            .slot_of(Duration::from_secs(block_timestamp))
            .unwrap_or_default();
        let target_slot = exit_slot + VOLUNTARY_EXIT_SLOTS_TO_POSTPONE;

        // Schedule the exit for processing at the target slot
        let scheduled = exit_tracker.add_duty_for_slot(
            target_slot,
            validator_pubkey,
            validator_index,
            is_our_validator,
        );

        if scheduled {
            info!(
                validator_pubkey = %validator_pubkey,
                validator_index = ?validator_index,
                current_slot = ?exit_slot,
                target_slot = ?target_slot,
                "Scheduled voluntary exit for processing"
            );
        } else {
            debug!(
                validator_pubkey = %validator_pubkey,
                validator_index = ?validator_index,
                "Exit tracked but not scheduled (not our validator)"
            );
        }
    }

    info!("Exit request receiver shutting down");
}

/// Processes scheduled exits at their target slots
async fn process_scheduled_exits<E: EthSpec, T: SlotClock + 'static>(
    slot_clock: impl SlotClock + 'static,
    slots_per_epoch: u64,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    exit_tracker: Arc<VoluntaryExitTracker>,
) {
    info!("Starting voluntary exit processor");

    loop {
        // Get current slot
        let current_slot = match slot_clock.now() {
            Some(slot) => slot,
            None => {
                debug!("Failed to get current slot, will retry");
                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        // Process all exits ready for this slot (including earlier slots)
        let ready_exits = exit_tracker.get_ready_exits(current_slot);

        if !ready_exits.is_empty() {
            debug!(
                slot = ?current_slot,
                num_exits = ready_exits.len(),
                "Processing scheduled voluntary exits"
            );

            for exit_duty in ready_exits {
                let success = process_single_exit(
                    slots_per_epoch,
                    beacon_nodes.clone(),
                    validator_store.clone(),
                    &exit_duty,
                )
                .await;

                exit_tracker.remove_processed_exit(&exit_duty);

                if success {
                    info!(
                        validator_index = ?exit_duty.validator_index,
                        "Successfully processed and removed voluntary exit"
                    );
                } else {
                    debug!(
                        validator_index = ?exit_duty.validator_index,
                        "Failed to process voluntary exit"
                    );
                }
            }
        }

        // Prune old slots from the tracker
        exit_tracker.prune(current_slot, slots_per_epoch);

        let sleep_duration = slot_clock.duration_to_next_slot().unwrap_or_else(|| {
            // If we can't read the slot clock, just wait one slot.
            slot_clock.slot_duration()
        });

        sleep(sleep_duration).await;
    }
}

/// Process a single exit
async fn process_single_exit<E: EthSpec, T: SlotClock + 'static>(
    slots_per_epoch: u64,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    exit_duty: &ExitDuty,
) -> bool {
    let ExitDuty {
        validator_pubkey,
        validator_index,
        target_slot,
    } = exit_duty;

    info!(
        validator_pubkey = %validator_pubkey,
        validator_index = ?validator_index,
        target_slot = ?target_slot,
        "Processing scheduled voluntary exit"
    );

    let epoch = target_slot.epoch(slots_per_epoch);

    let voluntary_exit = voluntary_exit::VoluntaryExit {
        epoch,
        validator_index: validator_index.0 as u64,
    };

    match validator_store
        .collect_voluntary_exit_partial_signatures(*validator_pubkey, voluntary_exit, *target_slot)
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
                    // metrics::inc_counter_vec(
                    //     &metrics::EXECUTION_EVENTS_PROCESSED,
                    //     &["validator_exited"],
                    // );
                    true // Exit processed successfully
                }
                Err(e) => {
                    error!(
                        validator_pubkey = %validator_pubkey,
                        error = %e,
                        "Failed to submit voluntary exit to beacon node"
                    );
                    false // Exit not processed successfully
                }
            }
        }
        Err(e) => {
            error!(
                validator_pubkey = %validator_pubkey,
                error = ?e,
                "Failed to collect signatures for validator exit"
            );
            false // Exit not processed successfully
        }
    }
}
