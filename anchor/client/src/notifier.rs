use std::sync::Arc;

use anchor_validator_store::AnchorValidatorStore;
use database::NetworkState;
use operator_doppelganger::OperatorDoppelgangerService;
use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::{
    sync::watch,
    time::{Duration, sleep},
};
use tracing::{error, info};
use types::{ChainSpec, EthSpec};

use crate::duties_service::DutiesService;

/// Represents whether the client is synced with the execution layer
#[derive(Debug, Clone, Copy)]
enum SyncState {
    Syncing,
    Synced,
}

impl SyncState {
    fn from_bool(is_synced: bool) -> Self {
        if is_synced {
            Self::Synced
        } else {
            Self::Syncing
        }
    }
}

/// Represents the operator's presence on chain
#[derive(Debug, Clone, Copy)]
enum OperatorState {
    NoOperator,
    OperatorPresent {
        operator_id: ssv_types::OperatorId,
        cluster_count: usize,
    },
}

impl OperatorState {
    fn from_option(operator_id: Option<ssv_types::OperatorId>, cluster_count: usize) -> Self {
        if let Some(operator_id) = operator_id {
            Self::OperatorPresent {
                operator_id,
                cluster_count,
            }
        } else {
            Self::NoOperator
        }
    }
}

/// Represents the doppelgänger monitoring state
#[derive(Debug, Clone, Copy)]
enum DoppelgangerState {
    NotMonitoring,
    MonitoringForDoppelganger,
}

impl DoppelgangerState {
    fn from_service(doppelganger_service: Option<&Arc<OperatorDoppelgangerService>>) -> Self {
        if doppelganger_service
            .map(|service| service.is_monitoring())
            .unwrap_or(false)
        {
            Self::MonitoringForDoppelganger
        } else {
            Self::NotMonitoring
        }
    }
}

pub fn spawn_notifier<E: EthSpec, T: SlotClock + 'static>(
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    network_state: watch::Receiver<NetworkState>,
    synced: watch::Receiver<bool>,
    doppelganger_service: Option<Arc<OperatorDoppelgangerService>>,
    executor: TaskExecutor,
    spec: &ChainSpec,
) {
    let slot_duration = Duration::from_secs(spec.seconds_per_slot);

    let interval_fut = async move {
        loop {
            if let Some(duration_to_next_slot) = duties_service.slot_clock.duration_to_next_slot() {
                // Sleep until the middle of the next slot
                sleep(duration_to_next_slot + slot_duration / 2).await;
                notify(
                    &duties_service,
                    &network_state,
                    &synced,
                    doppelganger_service.as_ref(),
                )
                .await;
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

async fn notify<E: EthSpec, T: SlotClock + 'static>(
    duties_service: &DutiesService<AnchorValidatorStore<T, E>, T>,
    network_state: &watch::Receiver<NetworkState>,
    synced: &watch::Receiver<bool>,
    doppelganger_service: Option<&Arc<OperatorDoppelgangerService>>,
) {
    // Gather state information
    let (operator_id, cluster_count) = {
        let state = network_state.borrow();
        let operator_id = state.get_own_id();
        let cluster_count = state.get_own_clusters().len();
        (operator_id, cluster_count)
    };

    let validator_count = duties_service.total_validator_count() as i64;
    validator_metrics::set_gauge(
        &validator_metrics::ENABLED_VALIDATORS_COUNT,
        validator_count,
    );
    validator_metrics::set_gauge(&validator_metrics::TOTAL_VALIDATORS_COUNT, validator_count);

    let is_synced = *synced.borrow();

    // Build layered state
    let sync = SyncState::from_bool(is_synced);
    let operator = OperatorState::from_option(operator_id, cluster_count);
    let doppelganger = DoppelgangerState::from_service(doppelganger_service);

    // Match on compositional state layers
    match (&sync, &operator, &doppelganger, validator_count) {
        (SyncState::Syncing, OperatorState::NoOperator, _, _) => {
            info!("Syncing")
        }
        (
            SyncState::Syncing,
            OperatorState::OperatorPresent {
                operator_id,
                cluster_count: _,
            },
            _,
            _,
        ) => {
            info!(%operator_id, "Operator present on chain, waiting for sync")
        }
        (SyncState::Synced, OperatorState::NoOperator, _, _) => {
            info!("Synced, waiting for operator key to appear on chain")
        }
        (
            SyncState::Synced,
            OperatorState::OperatorPresent {
                operator_id,
                cluster_count,
            },
            DoppelgangerState::MonitoringForDoppelganger,
            count,
        ) if count > 0 => {
            info!(
                %operator_id,
                cluster_count,
                validator_count = count,
                "Monitoring for operator doppelgänger (duties paused)"
            )
        }
        (
            SyncState::Synced,
            OperatorState::OperatorPresent {
                operator_id,
                cluster_count,
            },
            DoppelgangerState::NotMonitoring,
            count,
        ) if count > 0 => {
            info!(%operator_id, cluster_count, "Operator active");
            // Only call Lighthouse's notifier when we're actually performing duties
            validator_services::notifier_service::notify(duties_service).await;
        }
        (
            SyncState::Synced,
            OperatorState::OperatorPresent {
                operator_id,
                cluster_count: _,
            },
            _,
            _,
        ) => {
            info!(%operator_id, "Operator ready, no validators assigned")
        }
    }
}
