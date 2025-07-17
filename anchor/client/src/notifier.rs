use std::sync::Arc;

use anchor_validator_store::AnchorValidatorStore;
use database::NetworkState;
use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::{
    sync::watch,
    time::{Duration, sleep},
};
use tracing::{error, info};
use types::{ChainSpec, EthSpec};

use crate::duties_service::DutiesService;

/// Spawns a notifier service which periodically logs information about the node. It reuses
/// Lighthouse's `notifier_service` for most functionality but adds some Anchor-specific information
/// and logic.
///
/// Executes [`notify`] once per slot, halfway into it.
pub fn spawn_notifier<E: EthSpec, T: SlotClock + 'static>(
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    network_state: watch::Receiver<NetworkState>,
    synced: watch::Receiver<bool>,
    executor: TaskExecutor,
    spec: &ChainSpec,
) {
    let slot_duration = Duration::from_secs(spec.seconds_per_slot);

    let interval_fut = async move {
        loop {
            if let Some(duration_to_next_slot) = duties_service.slot_clock.duration_to_next_slot() {
                // Sleep until the middle of the next slot
                sleep(duration_to_next_slot + slot_duration / 2).await;
                notify(&duties_service, &network_state, &synced).await;
            } else {
                error!("Failed to read slot clock");
                // If we can't read the slot clock, just wait another slot.
                sleep(slot_duration).await;
                continue;
            }
        }
    };

    executor.spawn(interval_fut, "validator_notifier");
}

/// Performs a single notification routine.
///
/// This serves to notify the user of the current application status via `info` logging.
/// Additionally, some metrics are recorded by the `validator_services`.
async fn notify<E: EthSpec, T: SlotClock + 'static>(
    duties_service: &DutiesService<AnchorValidatorStore<T, E>, T>,
    network_state: &watch::Receiver<NetworkState>,
    synced: &watch::Receiver<bool>,
) {
    // Scope needed as Rust complains about `state` being held across `await` if using `drop`
    let (operator_id, cluster_count) = {
        let state = network_state.borrow();
        let operator_id = state.get_own_id();
        let cluster_count = state.get_own_clusters().len();
        (operator_id, cluster_count)
    };

    let is_synced = *synced.borrow();

    match (operator_id, is_synced) {
        (None, false) => info!("Syncing"),
        (None, true) => info!("Synced, waiting for operator key to appear on chain"),
        (Some(operator_id), false) => {
            info!(%operator_id, "Operator present on chain, waiting for sync")
        }
        (Some(operator_id), true) if duties_service.total_validator_count() > 0 => {
            info!(%operator_id, cluster_count, "Operator active");
            validator_services::notifier_service::notify(duties_service).await;
        }
        (Some(operator_id), true) => info!(%operator_id, "Operator ready, no validators assigned"),
    }
}
