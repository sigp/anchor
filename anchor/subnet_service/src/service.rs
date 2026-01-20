//! Subnet subscription service.
//!
//! This module provides the background service that manages subnet subscriptions
//! based on the clusters owned by the operator.

use std::{collections::HashSet, sync::Arc, time::Duration};

use database::{NetworkState, UniqueIndex};
use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::{
    sync::{mpsc, watch},
    time::sleep,
};
use tracing::{debug, error, warn};
use types::{ChainSpec, EthSpec};

use crate::{
    SubnetEvent, SubnetId, message_rate,
    scoring::{calculate_message_rate_for_subnet, get_committee_info_for_subnet},
};

/// Background service that manages subnet subscriptions.
///
/// This service monitors the database for cluster changes and emits `SubnetEvent`s
/// to notify the network layer about subnet subscriptions.
struct SubnetService<S: SlotClock> {
    tx: mpsc::Sender<SubnetEvent>,
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    disable_gossipsub_topic_scoring: bool,
    slot_clock: S,
    chain_spec: Arc<ChainSpec>,
    previous_subnets: HashSet<SubnetId>,
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
        slot_clock: S,
        chain_spec: Arc<ChainSpec>,
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
            previous_subnets,
        }
    }

    /// Main background task that manages subnet subscriptions and scoring updates.
    async fn run<E: EthSpec>(mut self) {
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
    async fn run_scoring_loop<E: EthSpec>(&mut self) {
        loop {
            sleep(calculate_duration_to_next_epoch::<E>(&self.slot_clock)).await;
            self.send_scoring_rate_updates::<E>().await;
        }
    }

    /// Monitor DB for subnet changes, with optional scoring updates at epoch boundaries.
    async fn run_monitoring_loop<E: EthSpec>(&mut self) {
        loop {
            let delay = calculate_duration_to_next_epoch::<E>(&self.slot_clock);
            tokio::select! {
                _ = self.db.changed() => {
                    self.handle_subnet_changes::<E>().await;
                }
                _ = sleep(delay), if !self.disable_gossipsub_topic_scoring => {
                    self.send_scoring_rate_updates::<E>().await;
                }
            }
        }
    }

    /// Send initial Join events for all subnets. Returns Err if the channel closed.
    async fn send_initial_joins<E: EthSpec>(&mut self) -> Result<(), ()> {
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
    async fn handle_subnet_changes<E: EthSpec>(&mut self) {
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

        // For every subnet that was previously joined but is no longer in current_subnets,
        // send a Leave event.
        for subnet in self.previous_subnets.difference(&current_subnets) {
            debug!(?subnet, "send leave");
            if self.tx.send(SubnetEvent::Leave(*subnet)).await.is_err() {
                warn!("Network no longer listening for subnets");
                return;
            }
        }

        // For every subnet that was not previously joined but is now in current_subnets,
        // send a Join event.
        for subnet in current_subnets.difference(&self.previous_subnets) {
            debug!(?subnet, "send join");
            let message_rate = {
                let state = self.db.borrow();
                self.subnet_message_rate::<E>(subnet, &state)
            };

            if self
                .tx
                .send(SubnetEvent::Join(*subnet, message_rate))
                .await
                .is_err()
            {
                warn!("Network no longer listening for subnets");
                return;
            }
        }

        // Update the previous_subnets for next iteration
        self.previous_subnets = current_subnets;
    }

    /// Emit updated message-rate estimates for gossipsub topic scoring.
    ///
    /// Gossipsub uses these rates to set per-topic scoring parameters that detect:
    /// - Flooding (too many messages vs expected)
    /// - Underperformance (too few messages vs expected)
    ///
    /// Rates are recalculated at each epoch because committee compositions and
    /// sync committee memberships can change.
    async fn send_scoring_rate_updates<E: EthSpec>(&mut self) {
        debug!(
            subnet_count = self.previous_subnets.len(),
            "Sending updated scoring rates for all subnets"
        );

        for &subnet in &self.previous_subnets {
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

/// Spawn the subnet service task and return the receiver for subnet events.
#[allow(clippy::too_many_arguments)]
pub fn start_subnet_service<E: EthSpec>(
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    disable_gossipsub_topic_scoring: bool,
    executor: &TaskExecutor,
    slot_clock: impl SlotClock + 'static,
    chain_spec: Arc<ChainSpec>,
) -> mpsc::Receiver<SubnetEvent> {
    let (tx, rx) = mpsc::channel(if subscribe_all_subnets {
        subnet_count
    } else {
        1
    });

    let service = SubnetService::new(
        tx,
        db,
        subnet_count,
        subscribe_all_subnets,
        disable_gossipsub_topic_scoring,
        slot_clock,
        chain_spec,
    );

    executor.spawn(service.run::<E>(), "subnet_service");

    rx
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
