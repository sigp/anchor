use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use database::{NetworkState, UniqueIndex};
use fork::{Fork, ForkConfig, ForkLifecycle};
use slot_clock::SlotClock;
use ssv_types::OperatorId;
use tokio::{sync::watch, time::sleep};
use tracing::{debug, error, warn};
use types::EthSpec;

use crate::{
    SUBNET_COUNT, SubnetId, SubnetServiceError, TopicEvent, service::SubnetService,
    topic::create_topic,
};

pub(crate) struct ServiceState {
    pub(crate) forks: HashMap<Fork, ForkSubscriptions>,
    pub(crate) fork_to_score: Fork,
}

impl ServiceState {
    fn set_subscribed_forks<const N: usize>(&mut self, forks: [ForkConfig; N]) {
        // Remove forks that we should no longer be subscribed to.
        self.forks
            .retain(|fork, _| forks.iter().any(|f| f.fork == *fork));
        // Ensure we track subscriptions for all forks that we should be subscribed to.
        for fork in forks {
            self.forks.entry(fork.fork).or_insert(ForkSubscriptions {
                config: fork.clone(),
                currently_subscribed: HashSet::new(),
            });
        }
    }
}

pub(crate) struct ForkSubscriptions {
    pub(crate) config: ForkConfig,
    pub(crate) currently_subscribed: HashSet<SubnetId>,
}

enum SubnetGenerator {
    All,
    Committees(HashSet<Vec<OperatorId>>),
}

impl SubnetGenerator {
    fn committees_from_network_state(state: &NetworkState) -> Self {
        let all_committee_members = state
            .get_own_clusters()
            .iter()
            .flat_map(|cluster| state.clusters().get_by(cluster))
            .map(|cluster| cluster.cluster_members.iter().copied().collect::<Vec<_>>())
            .collect();
        SubnetGenerator::Committees(all_committee_members)
    }

    fn generate_subnets(&self, fork: Fork) -> Result<HashSet<SubnetId>, SubnetServiceError> {
        let subnets = match self {
            SubnetGenerator::All => (0..SUBNET_COUNT)
                .map(|subnet_id| SubnetId::new(subnet_id as u64))
                .collect(),
            SubnetGenerator::Committees(committees) => committees
                .iter()
                .map(|committee| {
                    SubnetId::from_operators_for_fork(committee, fork)
                        .map_err(SubnetServiceError::SubnetCalculation)
                })
                .collect::<Result<_, _>>()?,
        };
        Ok(subnets)
    }
}

impl<S: SlotClock> SubnetService<S> {
    /// Main background task that manages subnet subscriptions and scoring updates.
    ///
    /// This method takes `Arc<Self>` to allow the service to be shared while running.
    pub async fn run<E: EthSpec>(
        self: Arc<Self>,
        mut lifecycle_rx: watch::Receiver<ForkLifecycle>,
    ) {
        let mut db = self.db.clone();
        let mut service_state = self.initial_service_state::<E>(&mut lifecycle_rx).await;

        loop {
            let delay = calculate_duration_to_next_epoch::<E>(&*self.slot_clock);
            tokio::select! {
                _ = db.changed(), if !self.subscribe_all_subnets => {
                    self.handle_subnet_changes::<E>(&mut service_state).await;
                }
                _ = sleep(delay), if !self.disable_gossipsub_topic_scoring => {
                    self.send_scoring_rate_updates::<E>(&service_state).await;
                }
                Ok(()) = lifecycle_rx.changed() => {
                    let new_lifecycle = lifecycle_rx.borrow_and_update().clone();
                    self.on_lifecycle_transition(new_lifecycle, &mut service_state);
                }
            }
        }
    }

    /// Handle a lifecycle state transition by updating the service's fork subscriptions.
    ///
    /// Compares the previous and new lifecycle states and performs the corresponding
    /// subscription management:
    /// - Normal → WarmUp: insert upcoming fork (subscribe to new topics)
    /// - WarmUp/Normal → GracePeriod: update fork_to_score, insert current fork
    /// - GracePeriod → Normal: remove & unsubscribe previous fork
    /// - GracePeriod → WarmUp: remove previous + insert upcoming (overlapping transition)
    fn on_lifecycle_transition(&self, new: ForkLifecycle, service_state: &mut ServiceState) {
        service_state.fork_to_score = new.current_fork_config().fork;
        match new {
            ForkLifecycle::Normal { current } => {
                service_state.set_subscribed_forks([current]);
            }
            ForkLifecycle::WarmUp { current, upcoming } => {
                service_state.set_subscribed_forks([current, upcoming]);
            }
            ForkLifecycle::GracePeriod { current, previous } => {
                service_state.set_subscribed_forks([current, previous]);
            }
        }
    }

    async fn initial_service_state<E: EthSpec>(
        &self,
        lifecycle_rx: &mut watch::Receiver<ForkLifecycle>,
    ) -> ServiceState {
        let mut service_state = ServiceState {
            forks: HashMap::new(),
            // This will be updated by the `on_lifecycle_transition` call below.
            fork_to_score: Fork::Alan,
        };

        let initial_lifecycle = lifecycle_rx.borrow_and_update().clone();
        self.on_lifecycle_transition(initial_lifecycle, &mut service_state);

        self.handle_subnet_changes::<E>(&mut service_state).await;

        service_state
    }

    /// Compare current and previous subnets, emitting subscribe/unsubscribe events.
    ///
    /// High-level flow:
    /// 1) Build the current subnet set + topic prefix for the current slot.
    /// 2) Diff against the last recorded subnets to emit unsubscribes/subscribes.
    /// 3) If a fork transition is active, manage transition topics in parallel.
    ///
    /// The transition path is careful to avoid duplicate subscriptions when the
    /// transition prefix matches the current prefix.
    async fn handle_subnet_changes<E: EthSpec>(&self, service_state: &mut ServiceState) {
        let subnet_source = if self.subscribe_all_subnets {
            SubnetGenerator::All
        } else {
            let state = self.db.borrow();
            SubnetGenerator::committees_from_network_state(&state)
        };

        for fork in service_state.forks.values_mut() {
            let score = fork.config.fork == service_state.fork_to_score;
            if let Err(err) = self
                .handle_subnet_changes_for_fork::<E>(&subnet_source, fork, score)
                .await
            {
                error!(fork=?fork.config.fork, ?err, "Failed to handle subnet changes for fork")
            }
        }
    }

    async fn handle_subnet_changes_for_fork<E: EthSpec>(
        &self,
        subnet_generator: &SubnetGenerator,
        fork: &mut ForkSubscriptions,
        score: bool,
    ) -> Result<(), SubnetServiceError> {
        let target_subnets = subnet_generator.generate_subnets(fork.config.fork)?;

        let to_leave = fork
            .currently_subscribed
            .difference(&target_subnets)
            .copied();
        let to_join = target_subnets
            .difference(&fork.currently_subscribed)
            .copied();

        self.send_unsubscribes(&fork.config, to_leave).await?;
        self.send_subscribes::<E>(&fork.config, to_join, score)
            .await?;

        fork.currently_subscribed = target_subnets;
        Ok(())
    }

    /// Emit unsubscribe events for the given prefix/subnets.
    async fn send_unsubscribes<I>(
        &self,
        fork_config: &ForkConfig,
        subnets: I,
    ) -> Result<(), SubnetServiceError>
    where
        I: IntoIterator<Item = SubnetId>,
    {
        let prefix = fork_config
            .fork
            .topic_prefix(self.router().fork_schedule().network_name());
        for subnet in subnets {
            let topic = create_topic(&prefix, subnet);
            debug!(%topic, "send unsubscribe");
            if self
                .tx
                .send(TopicEvent::Unsubscribe { topic, subnet })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
                return Err(SubnetServiceError::SendFailed);
            }
        }
        Ok(())
    }

    /// Emit subscribe events for current subnets. If `send_message_rate` is true, also emit
    /// scoring rate events, unless they are disabled.
    async fn send_subscribes<E: EthSpec>(
        &self,
        fork_config: &ForkConfig,
        subnets: impl IntoIterator<Item = SubnetId>,
        send_message_rate: bool,
    ) -> Result<(), SubnetServiceError> {
        let prefix = fork_config
            .fork
            .topic_prefix(self.router().fork_schedule().network_name());
        for subnet in subnets {
            let topic = create_topic(&prefix, subnet);
            debug!(%topic, "send subscribe");
            let message_rate = send_message_rate
                .then(|| {
                    let state = self.db.borrow();
                    self.subnet_message_rate::<E>(&subnet, fork_config, &state)
                })
                .flatten();

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
                return Err(SubnetServiceError::SendFailed);
            }
        }
        Ok(())
    }
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
