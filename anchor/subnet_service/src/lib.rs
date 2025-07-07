use std::{collections::HashSet, ops::Deref, time::Duration};

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
use types::EthSpec;

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
    Join(SubnetId, Vec<CommitteeInfo>),
    Leave(SubnetId),
    /// Committee information has changed for an already-joined subnet
    CommitteeUpdate(SubnetId, Vec<CommitteeInfo>),
}

pub fn start_subnet_service<E: EthSpec>(
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    executor: &TaskExecutor,
    slot_clock: impl SlotClock + 'static,
) -> mpsc::Receiver<SubnetEvent> {
    if !subscribe_all_subnets {
        // a channel capacity of 1 is fine - the subnet_service does not do anything else, it can
        // wait.
        let (tx, rx) = mpsc::channel(1);
        executor.spawn(
            subnet_tracker::<E>(tx, db, subnet_count, slot_clock),
            "subnet_service",
        );
        rx
    } else {
        let (tx, rx) = mpsc::channel(subnet_count);
        for subnet in (0..(subnet_count as u64)).map(SubnetId) {
            // For the "all subnets" case, we don't have specific committee info, so pass an empty
            // vec
            if let Err(err) = tx.try_send(SubnetEvent::Join(subnet, Vec::new())) {
                error!(?err, "Impossible error while subscribing to all subnets");
            }
        }
        rx
    }
}

/// The main background task:
/// - Gathers the current subnets from `NetworkState`.
/// - Compares them to the previously-seen subnets.
/// - Emits `Join` events for newly-added subnets and `Leave` events for removed subnets.
/// - Recalculates topic scores for all subnets at epoch boundaries.
async fn subnet_tracker<E: EthSpec>(
    tx: mpsc::Sender<SubnetEvent>,
    mut db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    slot_clock: impl SlotClock,
) {
    // `previous_subnets` tracks which subnets were joined in the last iteration.
    let mut previous_subnets = HashSet::new();

    // Calculate duration until the first epoch boundary
    let mut next_epoch_delay = calculate_seconds_to_next_epoch::<E>(&slot_clock);

    loop {
        tokio::select! {
            // Handle database changes for subnet join/leave
            _ = db.changed() => {
                handle_subnet_changes(&tx, &mut db, &mut previous_subnets, subnet_count).await;
            }

            // Handle scheduled epoch boundaries
            _ = sleep(next_epoch_delay) => {
                if let Some(current_slot) = slot_clock.now() {
                    let current_epoch = current_slot.epoch(E::slots_per_epoch());
                    debug!(
                        epoch = current_epoch.as_u64(),
                        "Epoch boundary reached - recalculating topic scores for all subnets"
                    );
                    handle_epoch_committee_update(&tx, &mut db, &previous_subnets).await;

                    // Schedule the next epoch boundary (one full epoch from now)
                    let epoch_duration = slot_clock.slot_duration() * E::slots_per_epoch() as u32;
                    next_epoch_delay = epoch_duration;
                } else {
                    // If we can't get current slot, recalculate the delay
                    warn!("Could not get current slot during epoch boundary, recalculating delay");
                    next_epoch_delay = calculate_seconds_to_next_epoch::<E>(&slot_clock);
                }
            }
        }
    }
}

/// Calculate duration until the next epoch boundary
fn calculate_seconds_to_next_epoch<E: EthSpec>(slot_clock: &impl SlotClock) -> Duration {
    if let Some(current_slot) = slot_clock.now() {
        let slot_duration = slot_clock.slot_duration();
        let slots_per_epoch = E::slots_per_epoch();

        // Calculate the current position within the epoch
        let current_slot_in_epoch = current_slot.as_u64() % slots_per_epoch;
        let remaining_slots_in_epoch = if current_slot_in_epoch == 0 {
            // We're at epoch boundary, next epoch is one full epoch away
            slots_per_epoch
        } else {
            // Calculate slots remaining in current epoch
            slots_per_epoch - current_slot_in_epoch
        };

        // Calculate time to next epoch boundary
        slot_duration * remaining_slots_in_epoch as u32
    } else {
        // Fallback: if we can't get current slot, use a conservative short interval
        let slot_duration = slot_clock.slot_duration();
        warn!("Could not get current slot for epoch delay calculation, using fallback timing");
        slot_duration * 3 // Wait 3 slots before next check
    }
}

/// Handle subnet join/leave events when database changes
async fn handle_subnet_changes(
    tx: &mpsc::Sender<SubnetEvent>,
    db: &mut watch::Receiver<NetworkState>,
    previous_subnets: &mut HashSet<SubnetId>,
    subnet_count: usize,
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
        // Get current committee info for this subnet
        let committees_info = {
            let state = db.borrow();
            get_committee_info_for_subnet(subnet, &*state)
        };

        if tx
            .send(SubnetEvent::Join(*subnet, committees_info))
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
async fn handle_epoch_committee_update(
    tx: &mpsc::Sender<SubnetEvent>,
    db: &mut watch::Receiver<NetworkState>,
    current_subnets: &HashSet<SubnetId>,
) {
    debug!(
        subnet_count = current_subnets.len(),
        "Recalculating topic scores for all subnets"
    );

    // Recalculate topic scores for all currently joined subnets
    for &subnet in current_subnets {
        let committees_info = {
            let state = db.borrow();
            get_committee_info_for_subnet(&subnet, &*state)
        };

        if tx
            .send(SubnetEvent::CommitteeUpdate(subnet, committees_info))
            .await
            .is_err()
        {
            warn!("Network no longer listening for subnets");
            return;
        }
    }
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
