use std::{collections::HashSet, sync::Arc, time::Duration};

use fork::ForkPhase;
use slot_clock::SlotClock;
use tokio::{sync::mpsc, time::sleep};
use tracing::{debug, error, warn};
use types::{EthSpec, Slot};

use crate::{SubnetId, TopicEvent, service::SubnetService};

struct SubscriptionContext {
    subnets: HashSet<SubnetId>,
    current_topic_prefix: String,
    last_topic_prefix: String,
}

impl<S: SlotClock> SubnetService<S> {
    /// Main background task that manages subnet subscriptions and scoring updates.
    ///
    /// This method takes `Arc<Self>` to allow the service to be shared while running.
    pub async fn run<E: EthSpec>(self: Arc<Self>, fork_phase_rx: mpsc::Receiver<ForkPhase>) {
        if self.subscribe_all_subnets && self.send_initial_joins::<E>().await.is_err() {
            return;
        }

        self.run_event_loop::<E>(fork_phase_rx).await;
    }

    /// Unified event loop for subnet changes, fork phases, and scoring updates.
    async fn run_event_loop<E: EthSpec>(&self, mut fork_phase_rx: mpsc::Receiver<ForkPhase>) {
        let mut db = self.db.clone();
        let mut fork_transition_prefix: Option<String> = None;
        let mut fork_transition_slot: Option<Slot> = None;
        let mut fork_transition_subnets = HashSet::new();
        let mut last_prefix: Option<String> = None;

        loop {
            let delay = calculate_duration_to_next_epoch::<E>(&*self.slot_clock);
            tokio::select! {
                _ = db.changed(), if !self.subscribe_all_subnets => {
                    let next_fork_transition_prefix = fork_transition_prefix.clone();
                    let next_fork_transition_slot = fork_transition_slot;
                    self.handle_subnet_changes::<E>(
                        &mut last_prefix,
                        &mut fork_transition_prefix,
                        &mut fork_transition_subnets,
                        next_fork_transition_prefix,
                        next_fork_transition_slot,
                    ).await;
                }
                _ = sleep(delay), if !self.disable_gossipsub_topic_scoring => {
                    self.send_scoring_rate_updates::<E>().await;
                }
                phase = fork_phase_rx.recv() => {
                    let Some(phase) = phase else {
                        warn!("Fork phase channel closed");
                        return;
                    };

                    let (next_fork_transition_prefix, next_fork_transition_slot) = match phase {
                        ForkPhase::Preparing { upcoming } => {
                            let slot =
                                Self::slot_for_fork_config(&upcoming, self.router.slots_per_epoch());
                            (Some(upcoming.topic_prefix.clone()), Some(slot))
                        }
                        ForkPhase::Activated { previous, .. } => {
                            let slot =
                                Self::slot_for_fork_config(&previous, self.router.slots_per_epoch());
                            (Some(previous.topic_prefix.clone()), Some(slot))
                        }
                        ForkPhase::GracePeriodEnded { .. } => (None, None),
                    };

                    self.handle_subnet_changes::<E>(
                        &mut last_prefix,
                        &mut fork_transition_prefix,
                        &mut fork_transition_subnets,
                        next_fork_transition_prefix.clone(),
                        next_fork_transition_slot,
                    ).await;

                    fork_transition_slot = next_fork_transition_slot;
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
    ///
    /// High-level flow:
    /// 1) Build the current subnet set + topic prefix for the current slot.
    /// 2) Diff against the last recorded subnets to emit unsubscribes/subscribes.
    /// 3) If a fork transition is active, manage transition topics in parallel.
    ///
    /// The transition path is careful to avoid duplicate subscriptions when the
    /// transition prefix matches the current prefix.
    async fn handle_subnet_changes<E: EthSpec>(
        &self,
        last_prefix: &mut Option<String>,
        fork_transition_prefix: &mut Option<String>,
        fork_transition_subnets: &mut HashSet<SubnetId>,
        next_fork_transition_prefix: Option<String>,
        next_fork_transition_slot: Option<Slot>,
    ) {
        // Resolve the current slot; without it we cannot compute current topics.
        let Some(current_slot) = self.slot_clock.now() else {
            warn!("Failed to get current slot for subnet updates");
            return;
        };

        // Current subnet set and topic prefixes derived from the current slot.
        // `last_topic_prefix` is the prefix used for the last recorded subscriptions; it may
        // differ from the current prefix across fork transitions, so we use it for unsubscribes.
        let subscription_context = self.current_subscription_context(current_slot, last_prefix);

        if self
            .apply_current_subscriptions::<E>(
                &subscription_context,
                fork_transition_prefix,
                fork_transition_subnets,
                last_prefix,
            )
            .await
            .is_err()
        {
            return;
        }

        let _ = self
            .update_additional_fork_subscriptions(
                &subscription_context,
                fork_transition_prefix,
                fork_transition_subnets,
                next_fork_transition_prefix,
                next_fork_transition_slot,
            )
            .await;
    }

    /// Build the current subnet set and topic prefixes for subscription updates.
    fn current_subscription_context(
        &self,
        current_slot: Slot,
        last_prefix: &Option<String>,
    ) -> SubscriptionContext {
        let current_subnets = self.compute_subnets_for_slot(current_slot);
        let current_topic_prefix = self.router.topic_prefix_for_slot(current_slot);
        let last_topic_prefix = last_prefix
            .as_ref()
            .cloned()
            .unwrap_or_else(|| current_topic_prefix.clone());
        SubscriptionContext {
            subnets: current_subnets,
            current_topic_prefix,
            last_topic_prefix,
        }
    }

    /// Compute current subnet joins/leaves relative to the last recorded subnets.
    fn current_subnet_changes(
        &self,
        current_subnets: &HashSet<SubnetId>,
    ) -> (Vec<SubnetId>, Vec<SubnetId>) {
        let previous = self.previous_subnets.read();
        let to_leave: Vec<_> = previous.difference(current_subnets).copied().collect();
        let to_join: Vec<_> = current_subnets.difference(&previous).copied().collect();
        (to_leave, to_join)
    }

    /// Persist the current subnets and prefix for the next tick.
    fn record_subscription_state(
        &self,
        subscription_context: &SubscriptionContext,
        last_prefix: &mut Option<String>,
    ) {
        *self.previous_subnets.write() = subscription_context.subnets.clone();
        *last_prefix = Some(subscription_context.current_topic_prefix.clone());
    }

    /// Compute the transition subnets for a fork slot (if any).
    fn transition_subnets_at_slot(
        &self,
        next_fork_transition_slot: Option<Slot>,
    ) -> HashSet<SubnetId> {
        match next_fork_transition_slot {
            Some(slot) => self.compute_subnets_for_slot(slot),
            None => HashSet::new(),
        }
    }

    /// Apply current topic subscriptions and persist the latest prefix/subnets.
    async fn apply_current_subscriptions<E: EthSpec>(
        &self,
        subscription_context: &SubscriptionContext,
        fork_transition_prefix: &Option<String>,
        fork_transition_subnets: &HashSet<SubnetId>,
        last_prefix: &mut Option<String>,
    ) -> Result<(), ()> {
        // Diff current subnets against the last recorded set.
        let (to_leave, mut to_join) = self.current_subnet_changes(&subscription_context.subnets);
        if fork_transition_prefix.as_ref() == Some(&subscription_context.current_topic_prefix) {
            // If transition topics share the current prefix, avoid double-subscribing.
            to_join.retain(|subnet| !fork_transition_subnets.contains(subnet));
        }

        // Unsubscribe using the *last* prefix (topics may have changed across forks).
        self.send_unsubscribes(&subscription_context.last_topic_prefix, to_leave)
            .await?;
        // Subscribe using the *current* prefix (slot-based fork selection).
        self.send_current_subscribes::<E, _>(&subscription_context.current_topic_prefix, to_join)
            .await?;
        // Persist the current subnets/prefix for the next tick.
        self.record_subscription_state(subscription_context, last_prefix);

        Ok(())
    }

    /// Update additional fork topic subscriptions and persist the extra prefix state.
    async fn update_additional_fork_subscriptions(
        &self,
        subscription_context: &SubscriptionContext,
        fork_transition_prefix: &mut Option<String>,
        fork_transition_subnets: &mut HashSet<SubnetId>,
        next_fork_transition_prefix: Option<String>,
        next_fork_transition_slot: Option<Slot>,
    ) -> Result<(), ()> {
        // During the warm-up period we subscribe to the new fork topics, and after
        // the grace period we unsubscribe from the old fork topics.
        let current_fork_transition_subnets =
            self.transition_subnets_at_slot(next_fork_transition_slot);
        let prefix_changed =
            fork_transition_prefix.as_ref() != next_fork_transition_prefix.as_ref();
        let (mut to_leave_fork_transition, to_join_fork_transition) = self
            .transition_subnet_changes(
                &current_fork_transition_subnets,
                fork_transition_subnets,
                prefix_changed,
            );

        if let Some(old_prefix) = fork_transition_prefix.as_ref()
            && *old_prefix == subscription_context.current_topic_prefix
        {
            // Avoid unsubscribing transition topics that are still covered by the current prefix.
            to_leave_fork_transition
                .retain(|subnet| !subscription_context.subnets.contains(subnet));
        }

        if let Some(old_prefix) = fork_transition_prefix.as_ref() {
            // Tear down stale additional subscriptions.
            self.send_unsubscribes(old_prefix, to_leave_fork_transition)
                .await?;
        }

        if let Some(new_prefix) = next_fork_transition_prefix.as_ref() {
            // Establish any new additional subscriptions.
            self.send_transition_subscribes(new_prefix, to_join_fork_transition)
                .await?;
        }

        // Persist transition state for next tick.
        *fork_transition_subnets = current_fork_transition_subnets;
        *fork_transition_prefix = next_fork_transition_prefix;
        Ok(())
    }

    /// Compute additional fork subnet joins/leaves for warm-up/grace periods.
    fn transition_subnet_changes(
        &self,
        current_fork_transition_subnets: &HashSet<SubnetId>,
        fork_transition_subnets: &HashSet<SubnetId>,
        prefix_changed: bool,
    ) -> (Vec<SubnetId>, Vec<SubnetId>) {
        if prefix_changed {
            (
                fork_transition_subnets.iter().copied().collect(),
                current_fork_transition_subnets.iter().copied().collect(),
            )
        } else {
            let to_leave: Vec<_> = fork_transition_subnets
                .difference(current_fork_transition_subnets)
                .copied()
                .collect();
            let to_join: Vec<_> = current_fork_transition_subnets
                .difference(fork_transition_subnets)
                .copied()
                .collect();
            (to_leave, to_join)
        }
    }

    /// Emit unsubscribe events for the given prefix/subnets.
    async fn send_unsubscribes<I>(&self, prefix: &str, subnets: I) -> Result<(), ()>
    where
        I: IntoIterator<Item = SubnetId>,
    {
        for subnet in subnets {
            let topic = Self::topic_for_subnet_with_prefix(prefix, subnet);
            debug!(%topic, "send unsubscribe");
            if self
                .tx
                .send(TopicEvent::Unsubscribe { topic, subnet })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
                return Err(());
            }
        }
        Ok(())
    }

    /// Emit subscribe events for current subnets with message rate (if enabled).
    async fn send_current_subscribes<E: EthSpec, I>(
        &self,
        prefix: &str,
        subnets: I,
    ) -> Result<(), ()>
    where
        I: IntoIterator<Item = SubnetId>,
    {
        for subnet in subnets {
            let topic = Self::topic_for_subnet_with_prefix(prefix, subnet);
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
                return Err(());
            }
        }
        Ok(())
    }

    /// Emit subscribe events for transition subnets (no scoring rate).
    async fn send_transition_subscribes<I>(&self, prefix: &str, subnets: I) -> Result<(), ()>
    where
        I: IntoIterator<Item = SubnetId>,
    {
        for subnet in subnets {
            let topic = Self::topic_for_subnet_with_prefix(prefix, subnet);
            debug!(%topic, "send subscribe");
            if self
                .tx
                .send(TopicEvent::Subscribe {
                    topic,
                    subnet,
                    message_rate: None,
                })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
                return Err(());
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
