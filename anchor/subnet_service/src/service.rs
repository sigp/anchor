//! Subnet subscription service.
//!
//! This module provides the background service that manages subnet subscriptions
//! based on the clusters owned by the operator.

use std::sync::Arc;

use database::{NetworkState, NonUniqueIndex};
use fork::{Fork, ForkLifecycle, ForkSchedule};
use slot_clock::SlotClock;
use ssv_types::{CommitteeId, OperatorId};
use task_executor::TaskExecutor;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use types::{ChainSpec, EthSpec, Slot};

use crate::{SUBNET_COUNT, SubnetCalculationError, SubnetId, TopicEvent, TopicRouter};

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
    /// Error while sending subscribe or unsubscribe messages.
    #[error("failed to send subscription updates")]
    SendFailed,
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
    pub(crate) subscribe_all_subnets: bool,
    pub(crate) disable_gossipsub_topic_scoring: bool,
    pub(crate) slot_clock: Arc<S>,
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// Topic router - single source of truth for fork-aware topic routing.
    pub(crate) router: TopicRouter,
}

impl<S: SlotClock> SubnetService<S> {
    /// Create a new subnet service.
    #[expect(clippy::too_many_arguments)]
    fn new(
        tx: mpsc::Sender<TopicEvent>,
        db: watch::Receiver<NetworkState>,
        subscribe_all_subnets: bool,
        disable_gossipsub_topic_scoring: bool,
        slot_clock: Arc<S>,
        chain_spec: Arc<ChainSpec>,
        fork_schedule: Arc<ForkSchedule>,
        slots_per_epoch: u64,
    ) -> Self {
        let router = TopicRouter::new(fork_schedule, slots_per_epoch);

        Self {
            tx,
            db,
            subscribe_all_subnets,
            disable_gossipsub_topic_scoring,
            slot_clock,
            chain_spec,
            router,
        }
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
            Fork::Alan => Ok(SubnetId::from_committee_alan(committee_id, SUBNET_COUNT)),
            Fork::Boole => SubnetId::from_operators(operator_ids, crate::SUBNET_COUNT_NZ)
                .map_err(SubnetServiceError::SubnetCalculation),
        }
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
}

/// Spawn the subnet service task and return both the service (for subnet queries)
/// and the receiver for topic events.
#[expect(clippy::too_many_arguments)]
pub fn start_subnet_service<S: SlotClock + 'static, E: EthSpec>(
    db: watch::Receiver<NetworkState>,
    subscribe_all_subnets: bool,
    disable_gossipsub_topic_scoring: bool,
    executor: &TaskExecutor,
    slot_clock: S,
    chain_spec: Arc<ChainSpec>,
    fork_schedule: Arc<ForkSchedule>,
    lifecycle_rx: watch::Receiver<ForkLifecycle>,
) -> (Arc<SubnetService<S>>, mpsc::Receiver<TopicEvent>) {
    let (tx, rx) = mpsc::channel(SUBNET_COUNT);

    let service = Arc::new(SubnetService::new(
        tx,
        db,
        subscribe_all_subnets,
        disable_gossipsub_topic_scoring,
        Arc::new(slot_clock),
        chain_spec,
        fork_schedule,
        E::slots_per_epoch(),
    ));

    executor.spawn(service.clone().run::<E>(lifecycle_rx), "subnet_service");

    (service, rx)
}
