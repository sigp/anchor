use std::{collections::HashSet, ops::Deref, sync::Arc, time::Duration};

use alloy::primitives::ruint::aliases::U256;
use database::{NetworkState, NonUniqueIndex, UniqueIndex};
use serde::{Deserialize, Serialize};
use slot_clock::SlotClock;
use ssv_types::{CommitteeId, CommitteeInfo};
use task_executor::TaskExecutor;
use tokio::{
    sync::{mpsc, watch},
    time::sleep,
};
use tracing::{debug, error, warn};
use types::{ChainSpec, EthSpec};

pub mod message_rate;

pub const SUBNET_COUNT: usize = 128;
pub type SubnetBits = [u8; SUBNET_COUNT / 8];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubnetId(#[serde(with = "serde_utils::quoted_u64")] u64);

impl SubnetId {
    pub fn new(id: u64) -> Self {
        id.into()
    }

    pub fn from_committee(committee_id: CommitteeId, subnet_count: usize) -> Self {
        // Derive a numeric "committee ID" and convert to an index in [0..subnet_count].
        let id = U256::from_be_bytes(*committee_id);
        SubnetId(
            (id % U256::from(subnet_count))
                .try_into()
                .expect("modulo must be < subnet_count"),
        )
    }
}

impl From<u64> for SubnetId {
    fn from(x: u64) -> Self {
        Self(x)
    }
}

impl Deref for SubnetId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub enum SubnetEvent {
    Join(SubnetId, Option<f64>), // subnet_id and optional message_rate
    Leave(SubnetId),
    /// Message rate has changed for an already-joined subnet
    RateUpdate(SubnetId, f64), /* subnet_id and new message_rate (only emitted when scoring is
                                * enabled) */
}

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

    executor.spawn(
        subnet_service::<E>(
            tx,
            db,
            subnet_count,
            subscribe_all_subnets,
            disable_gossipsub_topic_scoring,
            slot_clock,
            chain_spec,
        ),
        "subnet_service",
    );

    rx
}

/// The main background task:
/// - Gathers the current subnets from `NetworkState`.
/// - Compares them to the previously-seen subnets.
/// - Emits `Join` events for newly-added subnets and `Leave` events for removed subnets.
/// - Recalculates topic scores for all subnets at epoch boundaries.
async fn subnet_service<E: EthSpec>(
    tx: mpsc::Sender<SubnetEvent>,
    mut db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    disable_gossipsub_topic_scoring: bool,
    slot_clock: impl SlotClock,
    chain_spec: Arc<ChainSpec>,
) {
    // If subscribe_all_subnets is true, initialize by joining all subnets
    if subscribe_all_subnets {
        let initial_events: Vec<_> = {
            let current_state = db.borrow();
            (0..(subnet_count as u64))
                .map(SubnetId)
                .map(|subnet| {
                    let message_rate = if disable_gossipsub_topic_scoring {
                        None
                    } else {
                        let committees_info =
                            get_committee_info_for_subnet(&subnet, &*current_state);
                        Some(message_rate::calculate_message_rate_for_topic::<E>(
                            &committees_info,
                            &chain_spec,
                        ))
                    };
                    (subnet, message_rate)
                })
                .collect()
        };

        for (subnet, message_rate) in initial_events {
            if let Err(err) = tx.send(SubnetEvent::Join(subnet, message_rate)).await {
                error!(
                    ?err,
                    subnet = *subnet,
                    "Failed to send subnet join event during initialization"
                );
                return; // If we can't send, the receiver is dropped, so exit
            }
        }

        // If scoring is disabled, we've sent all Join events and there's nothing more to do
        if disable_gossipsub_topic_scoring {
            debug!("All subnets joined and scoring disabled - subnet service task complete");
            return;
        }
    }

    // `previous_subnets` tracks which subnets were joined in the last iteration.
    // For subscribe_all_subnets, we track all subnets; otherwise, only the ones we're subscribed
    // to.
    let mut previous_subnets = if subscribe_all_subnets {
        (0..(subnet_count as u64)).map(SubnetId).collect()
    } else {
        HashSet::new()
    };

    // Calculate duration until the first epoch boundary
    let mut next_epoch_delay = calculate_duration_to_next_epoch::<E>(&slot_clock);

    loop {
        tokio::select! {
            // Handle database changes for subnet join/leave (only if not subscribe_all_subnets)
            _ = db.changed(), if !subscribe_all_subnets => {
                handle_subnet_changes::<E>(&tx, &mut db, &mut previous_subnets, subnet_count, &chain_spec, disable_gossipsub_topic_scoring).await;
            }

            // Handle scheduled epoch boundaries (for both modes, but only if scoring is enabled)
            _ = sleep(next_epoch_delay), if !disable_gossipsub_topic_scoring => {
                handle_epoch_committee_update::<E>(&tx, &mut db, &previous_subnets, &chain_spec).await;
                // Recalculate the next epoch delay only after we've processed the epoch boundary
                next_epoch_delay = calculate_duration_to_next_epoch::<E>(&slot_clock);
            }
        }
    }
}

/// Calculate duration until the next epoch boundary
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

/// Handle subnet join/leave events when database changes
async fn handle_subnet_changes<E: EthSpec>(
    tx: &mpsc::Sender<SubnetEvent>,
    db: &mut watch::Receiver<NetworkState>,
    previous_subnets: &mut HashSet<SubnetId>,
    subnet_count: usize,
    chain_spec: &ChainSpec,
    disable_gossipsub_topic_scoring: bool,
) {
    // Build the `current_subnets` set by examining the clusters we own.
    let mut current_subnets = HashSet::new();

    // Get current subnets from database
    {
        let state = db.borrow();
        for cluster_id in state.get_own_clusters() {
            if let Some(cluster) = state.clusters().get_by(cluster_id) {
                let subnet_id = SubnetId::from_committee(cluster.committee_id(), subnet_count);
                current_subnets.insert(subnet_id);
            }
        }
    }

    // For every subnet that was previously joined but is no longer in `current_subnets`,
    // send a `Leave` event.
    for subnet in previous_subnets.difference(&current_subnets) {
        debug!(?subnet, "send leave");
        if tx.send(SubnetEvent::Leave(*subnet)).await.is_err() {
            warn!("Network no longer listening for subnets");
            return;
        }
    }

    // For every subnet that was not previously joined but is now in `current_subnets`,
    // send a `Join` event.
    for subnet in current_subnets.difference(previous_subnets) {
        debug!(?subnet, "send join");
        // Calculate current message rate for this subnet (or None if scoring is disabled)
        let message_rate = if disable_gossipsub_topic_scoring {
            None
        } else {
            let state = db.borrow();
            Some(calculate_message_rate_for_subnet::<E>(
                subnet, &*state, chain_spec,
            ))
        };

        if tx
            .send(SubnetEvent::Join(*subnet, message_rate))
            .await
            .is_err()
        {
            warn!("Network no longer listening for subnets");
            return;
        }
    }

    // Update the previous_subnets for next iteration
    *previous_subnets = current_subnets;
}

/// Handle epoch-based committee updates for all currently joined subnets
async fn handle_epoch_committee_update<E: EthSpec>(
    tx: &mpsc::Sender<SubnetEvent>,
    db: &mut watch::Receiver<NetworkState>,
    current_subnets: &HashSet<SubnetId>,
    chain_spec: &ChainSpec,
) {
    debug!(
        subnet_count = current_subnets.len(),
        "Recalculating message rates for all subnets at epoch boundary"
    );

    // Recalculate message rates for all currently joined subnets
    for &subnet in current_subnets {
        let message_rate = {
            let state = db.borrow();
            calculate_message_rate_for_subnet::<E>(&subnet, &*state, chain_spec)
        };

        if tx
            .send(SubnetEvent::RateUpdate(subnet, message_rate))
            .await
            .is_err()
        {
            warn!("Network no longer listening for subnets");
            return;
        }
    }
}

/// Calculate message rate for a specific subnet from the current network state
pub fn calculate_message_rate_for_subnet<E: EthSpec>(
    subnet: &SubnetId,
    network_state: impl Deref<Target = NetworkState>,
    chain_spec: &ChainSpec,
) -> f64 {
    let committees_info = get_committee_info_for_subnet(subnet, network_state);
    message_rate::calculate_message_rate_for_topic::<E>(&committees_info, chain_spec)
}

/// Get committee info for a specific subnet from the current network state
///
/// This function retrieves clusters for the subnet and converts them to CommitteeInfo
/// which includes both the committee members and validator indices.
pub fn get_committee_info_for_subnet(
    subnet: &SubnetId,
    network_state: impl Deref<Target = NetworkState>,
) -> Vec<CommitteeInfo> {
    network_state
        .clusters()
        .values()
        .filter(|cluster| {
            let cluster_subnet = SubnetId::from_committee(cluster.committee_id(), SUBNET_COUNT);
            cluster_subnet == *subnet
        })
        .map(|cluster| {
            // Convert cluster to CommitteeInfo by getting validator indices
            let validator_indices = network_state
                .metadata()
                .get_all_by(&cluster.cluster_id)
                .flat_map(|metadata| metadata.index)
                .collect::<Vec<_>>();

            CommitteeInfo {
                committee_members: cluster.cluster_members.clone(),
                validator_indices,
            }
        })
        .collect()
}

/// only useful for testing - introduce feature flag?
pub fn test_tracker(
    executor: TaskExecutor,
    events: Vec<SubnetEvent>,
    msg_delay: Duration,
) -> mpsc::Receiver<SubnetEvent> {
    let (tx, rx) = mpsc::channel(1);

    executor.spawn(
        async move {
            for event in events {
                sleep(msg_delay).await;
                tx.send(event).await.unwrap();
            }
            while !tx.is_closed() {
                sleep(Duration::from_millis(100)).await;
            }
        },
        "test_subnet_tracker",
    );

    rx
}
