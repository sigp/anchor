//! Subnet subscription service.
//!
//! This module provides the background service that manages subnet subscriptions
//! based on the clusters owned by the operator.

use std::{collections::HashSet, sync::Arc};

use database::{NetworkState, NonUniqueIndex, UniqueIndex};
use fork::{Fork, ForkConfig, ForkPhase, ForkSchedule};
use parking_lot::RwLock;
use slot_clock::SlotClock;
use ssv_types::{CommitteeId, OperatorId};
use task_executor::TaskExecutor;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tracing::warn;
use types::{ChainSpec, Epoch, EthSpec, Slot};

use crate::{SubnetCalculationError, SubnetId, TopicEvent, TopicRouter};

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
    pub(crate) tx: mpsc::Sender<TopicEvent>,
    pub(crate) db: watch::Receiver<NetworkState>,
    pub(crate) subnet_count: usize,
    pub(crate) subscribe_all_subnets: bool,
    pub(crate) disable_gossipsub_topic_scoring: bool,
    pub(crate) slot_clock: Arc<S>,
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// Topic router - single source of truth for fork-aware topic routing.
    pub(crate) router: TopicRouter,
    /// Previous subnets - uses RwLock for interior mutability when shared via Arc.
    pub(crate) previous_subnets: RwLock<HashSet<SubnetId>>,
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
    /// This uses the current slot to select the active fork. It is suitable for
    /// local subscription decisions that are based on the node's current time.
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
        self.subnet_for_committee_with_operators_at_slot(committee_id, operator_ids, slot)
    }

    /// Calculate the subnet for a committee at a specific slot when operators are already known.
    ///
    /// Per SIP-43, message routing and validation are slot-based. Use this when
    /// the message slot is known to select the correct fork rules.
    ///
    /// # Arguments
    ///
    /// * `committee_id` - The committee ID (used for Alan fork algorithm)
    /// * `operator_ids` - The operator IDs in the committee
    /// * `slot` - The slot that determines which fork rules apply
    ///
    /// # Errors
    ///
    /// Returns an error if subnet calculation fails.
    pub fn subnet_for_committee_with_operators_at_slot(
        &self,
        committee_id: CommitteeId,
        operator_ids: &[OperatorId],
        slot: Slot,
    ) -> Result<SubnetId, SubnetServiceError> {
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
    /// This uses the current slot to select the active fork. Use
    /// `subnet_for_committee_at_slot` when the message slot is known.
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
        let operator_ids = self.operator_ids_for_committee(committee_id)?;
        self.subnet_for_committee_with_operators(committee_id, &operator_ids)
    }

    /// Calculate the subnet for a committee at a specific slot by looking up operators.
    ///
    /// This is the slot-based variant for message routing/validation when the message
    /// slot is known.
    pub fn subnet_for_committee_at_slot(
        &self,
        committee_id: CommitteeId,
        slot: Slot,
    ) -> Result<SubnetId, SubnetServiceError> {
        let operator_ids = self.operator_ids_for_committee(committee_id)?;
        self.subnet_for_committee_with_operators_at_slot(committee_id, &operator_ids, slot)
    }

    fn operator_ids_for_committee(
        &self,
        committee_id: CommitteeId,
    ) -> Result<Vec<OperatorId>, SubnetServiceError> {
        self.db
            .borrow()
            .clusters()
            .get_all_by(&committee_id)
            .next()
            .map(|cluster| cluster.cluster_members.iter().copied().collect())
            .ok_or(SubnetServiceError::ClusterNotFound(committee_id))
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

    pub(crate) fn slot_for_fork_config(config: &ForkConfig, slots_per_epoch: u64) -> Slot {
        Slot::new(config.epoch.as_u64() * slots_per_epoch)
    }

    pub(crate) fn topic_for_subnet_with_prefix(prefix: &str, subnet: SubnetId) -> String {
        format!("{}{}", prefix, *subnet)
    }

    pub(crate) fn compute_subnets_for_slot(&self, slot: Slot) -> HashSet<SubnetId> {
        if self.subscribe_all_subnets {
            return (0..self.subnet_count as u64).map(SubnetId::new).collect();
        }

        let mut subnets = HashSet::new();
        let state = self.db.borrow();
        for cluster_id in state.get_own_clusters() {
            if let Some(cluster) = state.clusters().get_by(cluster_id) {
                let operator_ids: Vec<OperatorId> =
                    cluster.cluster_members.iter().copied().collect();
                match self.subnet_for_committee_with_operators_at_slot(
                    cluster.committee_id(),
                    &operator_ids,
                    slot,
                ) {
                    Ok(subnet_id) => {
                        subnets.insert(subnet_id);
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

        subnets
    }

    /// Create a topic string for a subnet using the current slot.
    ///
    /// This is used internally when emitting topic events, where we need to determine
    /// the correct topic based on the current time.
    pub(crate) fn current_topic_for_subnet(&self, subnet: SubnetId) -> Option<String> {
        let slot = self.slot_clock.now()?;
        Some(self.router.topic_for_subnet_at_slot(subnet, slot))
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
    fork_phase_rx: mpsc::Receiver<ForkPhase>,
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

    executor.spawn(service.clone().run::<E>(fork_phase_rx), "subnet_service");

    (service, rx)
}
