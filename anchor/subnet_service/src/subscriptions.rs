use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use database::{NetworkState, UniqueIndex};
use fork::{Fork, ForkConfig, ForkPhase};
use slot_clock::SlotClock;
use ssv_types::OperatorId;
use tokio::time::sleep;
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
        mut fork_phase_rx: async_broadcast::Receiver<ForkPhase>,
    ) {
        let mut db = self.db.clone();
        let Ok(mut service_state) = self.initial_service_state() else {
            error!("Failed to create initial subnet service state");
            return;
        };

        self.handle_subnet_changes::<E>(&mut service_state).await;

        loop {
            let delay = calculate_duration_to_next_epoch::<E>(&*self.slot_clock);
            tokio::select! {
                _ = db.changed(), if !self.subscribe_all_subnets => {
                    self.handle_subnet_changes::<E>(&mut service_state).await;
                }
                _ = sleep(delay), if !self.disable_gossipsub_topic_scoring => {
                    self.send_scoring_rate_updates::<E>(&service_state).await;
                }
                phase = fork_phase_rx.recv() => {
                    let Ok(phase) = phase else {
                        warn!("Fork phase channel closed");
                        return;
                    };

                    match phase {
                        ForkPhase::Preparing { upcoming } => {
                            service_state.forks.insert(upcoming.fork, ForkSubscriptions {
                                config: upcoming,
                                currently_subscribed: HashSet::new(),
                            });
                        }
                        ForkPhase::Activated { current, .. } => {
                            service_state.fork_to_score = current.fork;
                            service_state.forks.entry(current.fork).or_insert_with(|| ForkSubscriptions {
                                config: current,
                                currently_subscribed: HashSet::new(),
                            });
                        }
                        ForkPhase::GracePeriodEnded { previous, .. } => {
                            if let Some(fork) = service_state.forks.remove(&previous.fork)
                                && let Err(err) = self.send_unsubscribes(
                                    &fork.config.topic_prefix,
                                    fork.currently_subscribed
                                ).await
                            {
                                error!(
                                    fork = ?previous.fork,
                                    ?err,
                                    "Failed to unsubscribe from forks of older fork"
                                )
                            }
                        },
                    };

                    self.handle_subnet_changes::<E>(&mut service_state).await;
                }
            }
        }
    }

    fn initial_service_state(&self) -> Result<ServiceState, ()> {
        let schedule = self.router().fork_schedule();
        let epoch = self
            .slot_clock
            .now_or_genesis()
            .ok_or(())?
            .epoch(self.router().slots_per_epoch());

        let mut forks = HashMap::new();

        // The current fork will be subscribed to and scored
        let current_fork_config = schedule.active_fork_config(epoch).clone();
        let current_fork = current_fork_config.fork;
        forks.insert(
            current_fork,
            ForkSubscriptions {
                config: current_fork_config,
                currently_subscribed: HashSet::new(),
            },
        );

        // The fork within the preparation period will be subscribed to but not scored
        let preparation_fork = schedule.active_fork_config(epoch + fork::FORK_PREPARATION_EPOCHS);
        if preparation_fork.fork != current_fork {
            forks.insert(
                preparation_fork.fork,
                ForkSubscriptions {
                    config: preparation_fork.clone(),
                    currently_subscribed: HashSet::new(),
                },
            );
        }

        Ok(ServiceState {
            forks,
            fork_to_score: current_fork,
        })
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

        self.send_unsubscribes(&fork.config.topic_prefix, to_leave)
            .await?;
        self.send_subscribes::<E>(&fork.config, to_join, score)
            .await?;

        fork.currently_subscribed = target_subnets;
        Ok(())
    }

    /// Emit unsubscribe events for the given prefix/subnets.
    async fn send_unsubscribes<I>(&self, prefix: &str, subnets: I) -> Result<(), SubnetServiceError>
    where
        I: IntoIterator<Item = SubnetId>,
    {
        for subnet in subnets {
            let topic = create_topic(prefix, subnet);
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
        for subnet in subnets {
            let topic = create_topic(&fork_config.topic_prefix, subnet);
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
