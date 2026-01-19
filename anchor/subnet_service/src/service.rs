//! Subnet subscription service.
//!
//! This module provides the background service that manages subnet subscriptions
//! based on the clusters owned by the operator.

use std::{collections::HashSet, sync::Arc, time::Duration};

use database::{NetworkState, UniqueIndex};
use fork::ForkSchedule;
use parking_lot::RwLock;
use slot_clock::SlotClock;
use ssv_types::{CommitteeId, OperatorId};
use task_executor::TaskExecutor;
use thiserror::Error;
use tokio::{
    sync::{mpsc, watch},
    time::sleep,
};
use tracing::{debug, error, warn};
use types::{ChainSpec, EthSpec};

use crate::{
    SubnetCalculationError, SubnetEvent, SubnetId, message_rate,
    scoring::{calculate_message_rate_for_subnet, get_committee_info_for_subnet},
    subnet,
};

/// Error when calculating subnet from slot clock.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SubnetServiceError {
    /// Could not read the current slot from the slot clock.
    #[error("slot clock unavailable")]
    SlotClockUnavailable,
    /// Error during subnet calculation.
    #[error("subnet calculation failed: {0}")]
    SubnetCalculation(#[from] SubnetCalculationError),
}

/// Background service that manages subnet subscriptions.
///
/// This service monitors the database for cluster changes and emits `SubnetEvent`s
/// to notify the network layer about subnet subscriptions. It also provides
/// fork-aware subnet calculation for message routing.
///
/// The service can be shared via `Arc` to allow other components (like message sender)
/// to query the correct subnet for a committee based on the current fork.
pub struct SubnetService<S: SlotClock> {
    tx: mpsc::Sender<SubnetEvent>,
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    disable_gossipsub_topic_scoring: bool,
    slot_clock: Arc<S>,
    chain_spec: Arc<ChainSpec>,
    fork_schedule: Arc<ForkSchedule>,
    /// Slots per epoch - used for epoch calculation without requiring EthSpec.
    slots_per_epoch: u64,
    /// Previous subnets - uses RwLock for interior mutability when shared via Arc.
    previous_subnets: RwLock<HashSet<SubnetId>>,
}

impl<S: SlotClock> SubnetService<S> {
    /// Create a new subnet service.
    #[allow(clippy::too_many_arguments)]
    fn new(
        tx: mpsc::Sender<SubnetEvent>,
        db: watch::Receiver<NetworkState>,
        subnet_count: usize,
        subscribe_all_subnets: bool,
        disable_gossipsub_topic_scoring: bool,
        slot_clock: Arc<S>,
        chain_spec: Arc<ChainSpec>,
        fork_schedule: Arc<ForkSchedule>,
        slots_per_epoch: u64,
    ) -> Self {
        let previous_subnets = if subscribe_all_subnets {
            (0..(subnet_count as u64)).map(SubnetId::new).collect()
        } else {
            HashSet::new()
        };

        Self {
            tx,
            db,
            subnet_count,
            subscribe_all_subnets,
            disable_gossipsub_topic_scoring,
            slot_clock,
            chain_spec,
            fork_schedule,
            slots_per_epoch,
            previous_subnets: RwLock::new(previous_subnets),
        }
    }

    /// Calculate the subnet for a committee based on the current fork.
    ///
    /// This is the primary public method for fork-aware subnet calculation.
    /// It determines the active fork from the current epoch and uses the
    /// appropriate algorithm:
    /// - **Alan fork**: Uses `committee_id % subnet_count`
    /// - **Boole fork**: Uses MinHash of operator IDs
    ///
    /// # Arguments
    ///
    /// * `committee_id` - Optional committee ID (derived from operators if None)
    /// * `operator_ids` - The operator IDs in the committee
    ///
    /// # Errors
    ///
    /// Returns an error if the slot clock is unavailable or if subnet calculation fails.
    pub fn subnet_for_committee(
        &self,
        committee_id: Option<CommitteeId>,
        operator_ids: &[OperatorId],
    ) -> Result<SubnetId, SubnetServiceError> {
        let slot = self
            .slot_clock
            .now()
            .ok_or(SubnetServiceError::SlotClockUnavailable)?;
        let epoch = slot.epoch(self.slots_per_epoch);
        let fork = self.fork_schedule.active_fork(epoch);
        Ok(subnet::subnet_for_committee(
            fork,
            committee_id,
            operator_ids,
        )?)
    }

    /// Main background task that manages subnet subscriptions and scoring updates.
    ///
    /// This method takes `Arc<Self>` to allow the service to be shared while running.
    pub async fn run<E: EthSpec>(self: Arc<Self>) {
        if self.subscribe_all_subnets {
            if self.send_initial_joins::<E>().await.is_err() {
                return;
            }

            // When subscribed to all subnets, no DB monitoring is needed (subnets never change).
            // If scoring is also disabled, there's no ongoing work - we're done.
            if self.disable_gossipsub_topic_scoring {
                debug!("All subnets joined and scoring disabled - subnet service task complete");
                return;
            }

            // Periodically update scoring rates to reflect clusters joining/leaving.
            self.run_scoring_loop::<E>().await;
        } else {
            self.run_monitoring_loop::<E>().await;
        }
    }

    /// Periodically send scoring rate updates at epoch boundaries.
    async fn run_scoring_loop<E: EthSpec>(self: &Arc<Self>) {
        loop {
            sleep(calculate_duration_to_next_epoch::<E>(&*self.slot_clock)).await;
            self.send_scoring_rate_updates::<E>().await;
        }
    }

    /// Monitor DB for subnet changes, with optional scoring updates at epoch boundaries.
    async fn run_monitoring_loop<E: EthSpec>(self: &Arc<Self>) {
        // Clone the watch receiver so we can call changed() on it
        let mut db = self.db.clone();
        loop {
            let delay = calculate_duration_to_next_epoch::<E>(&*self.slot_clock);
            tokio::select! {
                _ = db.changed() => {
                    self.handle_subnet_changes::<E>().await;
                }
                _ = sleep(delay), if !self.disable_gossipsub_topic_scoring => {
                    self.send_scoring_rate_updates::<E>().await;
                }
            }
        }
    }

    /// Send initial Join events for all subnets. Returns Err if the channel closed.
    async fn send_initial_joins<E: EthSpec>(&self) -> Result<(), ()> {
        let initial_events: Vec<_> = {
            let current_state = self.db.borrow();
            (0..self.subnet_count as u64)
                .map(|id| {
                    let subnet = SubnetId::new(id);
                    let rate = self.subnet_message_rate::<E>(&subnet, &current_state);
                    (subnet, rate)
                })
                .collect()
        };

        for (subnet, message_rate) in initial_events {
            if let Err(err) = self.tx.send(SubnetEvent::Join(subnet, message_rate)).await {
                error!(?err, subnet = *subnet, "Failed to send subnet join event");
                return Err(());
            }
        }

        Ok(())
    }

    /// Compare current and previous subnets, emitting join/leave events.
    async fn handle_subnet_changes<E: EthSpec>(&self) {
        let mut current_subnets = HashSet::new();

        // Get current subnets from database
        {
            let state = self.db.borrow();
            for cluster_id in state.get_own_clusters() {
                if let Some(cluster) = state.clusters().get_by(cluster_id) {
                    let subnet_id =
                        SubnetId::from_committee_alan(cluster.committee_id(), self.subnet_count);
                    current_subnets.insert(subnet_id);
                }
            }
        }

        // Get previous subnets under lock, then release lock before async operations
        let (to_leave, to_join): (Vec<SubnetId>, Vec<SubnetId>) = {
            let previous = self.previous_subnets.read();
            let to_leave: Vec<_> = previous.difference(&current_subnets).copied().collect();
            let to_join: Vec<_> = current_subnets.difference(&previous).copied().collect();
            (to_leave, to_join)
        };

        // For every subnet that was previously joined but is no longer in current_subnets,
        // send a Leave event.
        for subnet in to_leave {
            debug!(?subnet, "send leave");
            if self.tx.send(SubnetEvent::Leave(subnet)).await.is_err() {
                warn!("Network no longer listening for subnets");
                return;
            }
        }

        // For every subnet that was not previously joined but is now in current_subnets,
        // send a Join event.
        for subnet in to_join {
            debug!(?subnet, "send join");
            let message_rate = {
                let state = self.db.borrow();
                self.subnet_message_rate::<E>(&subnet, &state)
            };

            if self
                .tx
                .send(SubnetEvent::Join(subnet, message_rate))
                .await
                .is_err()
            {
                warn!("Network no longer listening for subnets");
                return;
            }
        }

        // Update the previous_subnets for next iteration
        *self.previous_subnets.write() = current_subnets;
    }

    /// Emit updated message-rate estimates for gossipsub topic scoring.
    ///
    /// Gossipsub uses these rates to set per-topic scoring parameters that detect:
    /// - Flooding (too many messages vs expected)
    /// - Underperformance (too few messages vs expected)
    ///
    /// Rates are recalculated at each epoch because committee compositions and
    /// sync committee memberships can change.
    async fn send_scoring_rate_updates<E: EthSpec>(&self) {
        // Clone the subnets to avoid holding lock during async operations
        let subnets: Vec<SubnetId> = {
            let previous = self.previous_subnets.read();
            debug!(
                subnet_count = previous.len(),
                "Sending updated scoring rates for all subnets"
            );
            previous.iter().copied().collect()
        };

        for subnet in subnets {
            let message_rate = {
                let state = self.db.borrow();
                calculate_message_rate_for_subnet::<E>(&subnet, &*state, &self.chain_spec)
            };

            if self
                .tx
                .send(SubnetEvent::RateUpdate(subnet, message_rate))
                .await
                .is_err()
            {
                warn!("Network no longer listening for subnets");
                return;
            }
        }
    }

    /// Compute a subnet's message rate if scoring is enabled.
    fn subnet_message_rate<E: EthSpec>(
        &self,
        subnet: &SubnetId,
        network_state: &NetworkState,
    ) -> Option<f64> {
        if self.disable_gossipsub_topic_scoring {
            return None;
        }

        let committees_info = get_committee_info_for_subnet(subnet, network_state);
        Some(message_rate::calculate_message_rate_for_topic::<E>(
            &committees_info,
            &self.chain_spec,
        ))
    }
}

/// Spawn the subnet service task and return both the service (for subnet queries)
/// and the receiver for subnet events.
#[allow(clippy::too_many_arguments)]
pub fn start_subnet_service<S: SlotClock + 'static, E: EthSpec>(
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    disable_gossipsub_topic_scoring: bool,
    executor: &TaskExecutor,
    slot_clock: S,
    chain_spec: Arc<ChainSpec>,
    fork_schedule: Arc<ForkSchedule>,
) -> (Arc<SubnetService<S>>, mpsc::Receiver<SubnetEvent>) {
    let (tx, rx) = mpsc::channel(if subscribe_all_subnets {
        subnet_count
    } else {
        1
    });

    let service = Arc::new(SubnetService::new(
        tx,
        db,
        subnet_count,
        subscribe_all_subnets,
        disable_gossipsub_topic_scoring,
        Arc::new(slot_clock),
        chain_spec,
        fork_schedule,
        E::slots_per_epoch(),
    ));

    executor.spawn(service.clone().run::<E>(), "subnet_service");

    (service, rx)
}

/// Calculate duration until the next epoch boundary.
fn calculate_duration_to_next_epoch<E: EthSpec>(slot_clock: &impl SlotClock) -> Duration {
    if let Some(duration_to_next_epoch) = slot_clock.duration_to_next_epoch(E::slots_per_epoch()) {
        duration_to_next_epoch
    } else {
        // Fallback: if we can't get current slot, use a conservative short interval
        let slot_duration = slot_clock.slot_duration();
        warn!("Could not get current slot for epoch delay calculation, using fallback timing");
        slot_duration * 3 // Wait 3 slots before next check
    }
}
