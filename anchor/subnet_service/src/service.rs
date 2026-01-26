//! Subnet subscription service.
//!
//! This module provides the background service that manages subnet subscriptions
//! based on the clusters owned by the operator.

use std::{collections::HashSet, sync::Arc, time::Duration};

use database::{NetworkState, NonUniqueIndex, UniqueIndex};
use fork::{Fork, ForkSchedule};
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
use types::{ChainSpec, Epoch, EthSpec, Slot};

use crate::{SubnetCalculationError, SubnetId, TopicEvent, TopicRouter, message_rate};

/// Error when calculating subnet from slot clock.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SubnetServiceError {
    /// Could not read the current slot from the slot clock.
    #[error("slot clock unavailable")]
    SlotClockUnavailable,
    /// The cluster was not found in the database.
    #[error("cluster not found: {0:?}")]
    ClusterNotFound(CommitteeId),
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
    tx: mpsc::Sender<TopicEvent>,
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    disable_gossipsub_topic_scoring: bool,
    slot_clock: Arc<S>,
    chain_spec: Arc<ChainSpec>,
    /// Topic router - single source of truth for fork-aware topic routing.
    router: TopicRouter,
    /// Previous subnets - uses RwLock for interior mutability when shared via Arc.
    previous_subnets: RwLock<HashSet<SubnetId>>,
}

impl<S: SlotClock> SubnetService<S> {
    /// Create a new subnet service.
    #[allow(clippy::too_many_arguments)]
    fn new(
        tx: mpsc::Sender<TopicEvent>,
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

        let router = TopicRouter::new(fork_schedule, slots_per_epoch);

        Self {
            tx,
            db,
            subnet_count,
            subscribe_all_subnets,
            disable_gossipsub_topic_scoring,
            slot_clock,
            chain_spec,
            router,
            previous_subnets: RwLock::new(previous_subnets),
        }
    }

    /// Calculate the subnet for a committee when operators are already known.
    ///
    /// This is the core algorithm for fork-aware subnet calculation. Use this
    /// when you already have the operator IDs (e.g., from cluster data) to avoid
    /// a database lookup.
    ///
    /// Fork-specific algorithms:
    /// - **Alan fork**: Uses `committee_id % subnet_count`
    /// - **Boole fork**: Uses MinHash of operator IDs
    ///
    /// # Arguments
    ///
    /// * `committee_id` - The committee ID (used for Alan fork algorithm)
    /// * `operator_ids` - The operator IDs in the committee
    ///
    /// # Errors
    ///
    /// Returns an error if the slot clock is unavailable or subnet calculation fails.
    pub fn subnet_for_committee_with_operators(
        &self,
        committee_id: CommitteeId,
        operator_ids: &[OperatorId],
    ) -> Result<SubnetId, SubnetServiceError> {
        let slot = self
            .slot_clock
            .now()
            .ok_or(SubnetServiceError::SlotClockUnavailable)?;
        let fork = self.router.active_fork_at_slot(slot);

        match fork {
            Fork::Alan => Ok(SubnetId::from_committee_alan(
                committee_id,
                crate::SUBNET_COUNT,
            )),
            Fork::Boole => SubnetId::from_operators(operator_ids, crate::SUBNET_COUNT_NZ)
                .map_err(SubnetServiceError::SubnetCalculation),
        }
    }

    /// Calculate the subnet for a committee by looking up operators from the database.
    ///
    /// This is a convenience method that performs a database lookup for the cluster's
    /// operator IDs before calculating the subnet. Use `subnet_for_committee_with_operators`
    /// if you already have the operator IDs to avoid the extra lookup.
    ///
    /// # Arguments
    ///
    /// * `committee_id` - The committee ID to calculate the subnet for
    ///
    /// # Errors
    ///
    /// Returns an error if the slot clock is unavailable, the cluster is not found,
    /// or if subnet calculation fails.
    pub fn subnet_for_committee(
        &self,
        committee_id: CommitteeId,
    ) -> Result<SubnetId, SubnetServiceError> {
        // Look up operator IDs from database
        let operator_ids: Vec<OperatorId> = self
            .db
            .borrow()
            .clusters()
            .get_all_by(&committee_id)
            .next()
            .map(|cluster| cluster.cluster_members.iter().copied().collect())
            .ok_or(SubnetServiceError::ClusterNotFound(committee_id))?;

        self.subnet_for_committee_with_operators(committee_id, &operator_ids)
    }

    /// Get the topic router for direct access to routing logic.
    ///
    /// The router is the single source of truth for fork-aware topic routing.
    /// Use this when you need access to multiple routing methods or want to
    /// avoid repeated function call overhead.
    pub fn router(&self) -> &TopicRouter {
        &self.router
    }

    /// Get the active fork for a given epoch.
    ///
    /// This exposes the fork schedule's active fork lookup for use by other components
    /// (e.g., message validator) that need to determine which fork rules apply.
    pub fn active_fork(&self, epoch: Epoch) -> Fork {
        self.router.active_fork(epoch)
    }

    /// Get the domain type for a given epoch.
    ///
    /// Returns the domain type of the fork that is active at the given epoch.
    /// This is useful for slot-based validation where the message's slot determines
    /// which fork's rules apply.
    pub fn domain_type_for_epoch(
        &self,
        epoch: Epoch,
    ) -> Option<ssv_types::domain_type::DomainType> {
        self.router.domain_type_for_epoch(epoch)
    }

    /// Get the number of slots per epoch.
    ///
    /// This is useful for converting slots to epochs in validation logic.
    pub fn slots_per_epoch(&self) -> u64 {
        self.router.slots_per_epoch()
    }

    /// Create a topic string for a given subnet and slot.
    ///
    /// Determines the correct fork for the slot's epoch and creates the full topic
    /// string using that fork's topic prefix.
    ///
    /// Per SIP-43, publishing should use the topic corresponding to the message's
    /// slot, not necessarily the current fork.
    pub fn topic_for_subnet_at_slot(&self, subnet: SubnetId, slot: Slot) -> String {
        self.router.topic_for_subnet_at_slot(subnet, slot)
    }

    /// Create a topic string for a subnet using the current slot.
    ///
    /// This is used internally when emitting topic events, where we need to determine
    /// the correct topic based on the current time.
    fn current_topic_for_subnet(&self, subnet: SubnetId) -> Option<String> {
        let slot = self.slot_clock.now()?;
        Some(self.router.topic_for_subnet_at_slot(subnet, slot))
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

    /// Send initial Subscribe events for all subnets. Returns Err if the channel closed.
    async fn send_initial_joins<E: EthSpec>(&self) -> Result<(), ()> {
        let initial_events: Vec<_> = {
            let current_state = self.db.borrow();
            (0..self.subnet_count as u64)
                .map(|id| {
                    let subnet = SubnetId::new(id);
                    let rate = self.subnet_message_rate::<E>(&subnet, &current_state);
                    let topic = self.current_topic_for_subnet(subnet);
                    (subnet, topic, rate)
                })
                .collect()
        };

        for (subnet, topic, message_rate) in initial_events {
            let Some(topic) = topic else {
                error!(subnet = *subnet, "Failed to get current topic for subnet");
                return Err(());
            };
            if let Err(err) = self
                .tx
                .send(TopicEvent::Subscribe {
                    topic: topic.clone(),
                    subnet,
                    message_rate,
                })
                .await
            {
                error!(?err, %topic, "Failed to send topic subscribe event");
                return Err(());
            }
        }

        Ok(())
    }

    /// Compare current and previous subnets, emitting subscribe/unsubscribe events.
    async fn handle_subnet_changes<E: EthSpec>(&self) {
        let mut current_subnets = HashSet::new();

        // Get current subnets from database
        {
            let state = self.db.borrow();
            for cluster_id in state.get_own_clusters() {
                if let Some(cluster) = state.clusters().get_by(cluster_id) {
                    let operator_ids: Vec<OperatorId> =
                        cluster.cluster_members.iter().copied().collect();
                    match self
                        .subnet_for_committee_with_operators(cluster.committee_id(), &operator_ids)
                    {
                        Ok(subnet_id) => {
                            current_subnets.insert(subnet_id);
                        }
                        Err(e) => {
                            warn!(
                                ?e,
                                committee_id = ?cluster.committee_id(),
                                "Failed to calculate subnet"
                            );
                        }
                    }
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
        // send an Unsubscribe event.
        for subnet in to_leave {
            let Some(topic) = self.current_topic_for_subnet(subnet) else {
                warn!(?subnet, "Failed to get topic for subnet to leave");
                continue;
            };
            debug!(%topic, "send unsubscribe");
            if self
                .tx
                .send(TopicEvent::Unsubscribe { topic, subnet })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
                return;
            }
        }

        // For every subnet that was not previously joined but is now in current_subnets,
        // send a Subscribe event.
        for subnet in to_join {
            let Some(topic) = self.current_topic_for_subnet(subnet) else {
                warn!(?subnet, "Failed to get topic for subnet to join");
                continue;
            };
            debug!(%topic, "send subscribe");
            let message_rate = {
                let state = self.db.borrow();
                self.subnet_message_rate::<E>(&subnet, &state)
            };

            if self
                .tx
                .send(TopicEvent::Subscribe {
                    topic,
                    subnet,
                    message_rate,
                })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
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
                "Sending updated scoring rates for all topics"
            );
            previous.iter().copied().collect()
        };

        for subnet in subnets {
            let Some(topic) = self.current_topic_for_subnet(subnet) else {
                warn!(?subnet, "Failed to get topic for scoring rate update");
                continue;
            };

            let rate = {
                let state = self.db.borrow();
                let committees_info = self.get_committee_info_for_subnet(&subnet, &state);
                message_rate::calculate_message_rate_for_topic::<E>(
                    &committees_info,
                    &self.chain_spec,
                )
            };

            if self
                .tx
                .send(TopicEvent::RateUpdate {
                    topic,
                    message_rate: rate,
                })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
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

        let committees_info = self.get_committee_info_for_subnet(subnet, network_state);
        Some(message_rate::calculate_message_rate_for_topic::<E>(
            &committees_info,
            &self.chain_spec,
        ))
    }

    /// Get committee info for all clusters on a specific subnet.
    ///
    /// This function retrieves clusters that map to the given subnet and converts
    /// them to `CommitteeInfo` which includes both committee members and validator indices.
    fn get_committee_info_for_subnet(
        &self,
        subnet: &SubnetId,
        network_state: &NetworkState,
    ) -> Vec<ssv_types::CommitteeInfo> {
        network_state
            .clusters()
            .values()
            .filter(|cluster| {
                let operator_ids: Vec<OperatorId> =
                    cluster.cluster_members.iter().copied().collect();
                match self
                    .subnet_for_committee_with_operators(cluster.committee_id(), &operator_ids)
                {
                    Ok(cluster_subnet) => cluster_subnet == *subnet,
                    Err(_) => false,
                }
            })
            .map(|cluster| {
                // Convert cluster to CommitteeInfo by getting validator indices
                let validator_indices = network_state
                    .metadata()
                    .get_all_by(&cluster.cluster_id)
                    .flat_map(|metadata| metadata.index)
                    .collect::<Vec<_>>();

                ssv_types::CommitteeInfo {
                    committee_members: cluster.cluster_members.clone(),
                    validator_indices,
                }
            })
            .collect()
    }
}

/// Spawn the subnet service task and return both the service (for subnet queries)
/// and the receiver for topic events.
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
) -> (Arc<SubnetService<S>>, mpsc::Receiver<TopicEvent>) {
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
