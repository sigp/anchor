use std::{collections::HashSet, num::NonZeroU64, ops::Deref, sync::Arc, time::Duration};

use alloy::primitives::ruint::aliases::U256;
use database::{NetworkState, NonUniqueIndex, UniqueIndex};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use slot_clock::SlotClock;
use ssv_types::{CommitteeId, CommitteeInfo, OperatorId};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubnetCalculationError {
    EmptyOperatorList,
    InvalidSubnetCount,
    SubnetIdOutOfRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubnetId(#[serde(with = "serde_utils::quoted_u64")] u64);

impl SubnetId {
    pub fn new(id: u64) -> Self {
        id.into()
    }

    /// Calculate subnet using committee ID (Alan fork algorithm)
    ///
    /// This is the pre-fork algorithm that derives the subnet from the committee ID.
    /// Algorithm: `committee_id % subnet_count`
    pub fn from_committee_alan(committee_id: CommitteeId, subnet_count: usize) -> Self {
        // Derive a numeric "committee ID" and convert to an index in [0..subnet_count].
        let id = U256::from_be_bytes(*committee_id);
        SubnetId(
            (id % U256::from(subnet_count))
                .try_into()
                .expect("modulo must be < subnet_count"),
        )
    }

    /// Calculate subnet using MinHash of operator IDs (new algorithm post-fork)
    ///
    /// When an operator participates in multiple different operator sets, MinHash
    /// increases the likelihood those sets map to the same subnet (if that operator
    /// has the minimum hash). This reduces the number of subnets each operator must
    /// monitor.
    ///
    /// Algorithm:
    /// 1. For each operator ID, encode as little-endian u64 (8 bytes)
    /// 2. SHA256 hash each encoded operator ID individually
    /// 3. Find the minimum hash value
    /// 4. Return min_hash % subnet_count
    ///
    /// # Errors
    ///
    /// - `SubnetCalculationError::EmptyOperatorList` if `operator_ids` is empty
    /// - `SubnetCalculationError::SubnetIdOutOfRange` if the modulo result cannot fit in `u64`
    pub fn from_operators(
        operator_ids: &[OperatorId],
        subnet_count: NonZeroU64,
    ) -> Result<Self, SubnetCalculationError> {
        let min_hash: [u8; 32] = operator_ids
            .iter()
            .copied()
            .map(|operator_id| Sha256::digest(operator_id.to_le_bytes()).into())
            .min()
            .ok_or(SubnetCalculationError::EmptyOperatorList)?;

        let id = U256::from_be_bytes(min_hash);
        let modulus = U256::from(subnet_count.get());

        // Safe: x % subnet_count is always < subnet_count, which is a u64.
        let subnet_id: u64 = (id % modulus)
            .try_into()
            .map_err(|_| SubnetCalculationError::SubnetIdOutOfRange)?;

        Ok(SubnetId(subnet_id))
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

    loop {
        // Calculate duration until the next epoch boundary
        let next_epoch_delay = calculate_duration_to_next_epoch::<E>(&slot_clock);

        tokio::select! {
            // Handle database changes for subnet join/leave (only if not subscribe_all_subnets)
            _ = db.changed(), if !subscribe_all_subnets => {
                handle_subnet_changes::<E>(&tx, &mut db, &mut previous_subnets, subnet_count, &chain_spec, disable_gossipsub_topic_scoring).await;
            }

            // Handle scheduled epoch boundaries (for both modes, but only if scoring is enabled)
            _ = sleep(next_epoch_delay), if !disable_gossipsub_topic_scoring => {
                handle_epoch_committee_update::<E>(&tx, &mut db, &previous_subnets, &chain_spec).await;
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
                let subnet_id = SubnetId::from_committee_alan(cluster.committee_id(), subnet_count);
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
            let cluster_subnet =
                SubnetId::from_committee_alan(cluster.committee_id(), SUBNET_COUNT);
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

#[cfg(test)]
mod tests {
    use ssv_types::OperatorId;

    use super::*;

    const SUBNET_COUNT_NZ: NonZeroU64 = NonZeroU64::new(SUBNET_COUNT as u64).unwrap();

    #[test]
    fn test_from_operators_minhash() {
        // Test case with operators [1,2,3,4]
        // Operator 1: SHA256(0x0100000000000000) =
        // 7c9fa136d4413fa6173637e883b6998d32e1d675f88cddff9dcbcf331820f4b8 Operator 2:
        // SHA256(0x0200000000000000) =
        // d86e8112f3c4c4442126f8e9f44f16867da487f29052bf91b810457db34209a4 Operator 3:
        // SHA256(0x0300000000000000) =
        // 35be322d094f9d154a8aba4733b8497f180353bd7ae7b0a15f90b586b549f28b Operator 4:
        // SHA256(0x0400000000000000) =
        // f0a0278e4372459cca6159cd5e71cfee638302a7b9ca9b05c34181ac0a65ac5d Min hash is from
        // operator 3, so subnet = min_hash % 128
        let operators = vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];

        let subnet =
            SubnetId::from_operators(&operators, SUBNET_COUNT_NZ).expect("valid operators");

        // Calculate expected: operator 3's hash is smallest
        // 0x35be322d094f9d154a8aba4733b8497f180353bd7ae7b0a15f90b586b549f28b % 128
        // = 11 (from the big-endian modulo)
        assert_eq!(*subnet, 11);
    }

    #[test]
    fn test_from_operators_empty() {
        let operators = vec![];
        let result = SubnetId::from_operators(&operators, SUBNET_COUNT_NZ);
        assert_eq!(result, Err(SubnetCalculationError::EmptyOperatorList));
    }

    #[test]
    fn test_from_operators_single() {
        let operators = vec![OperatorId(42)];
        let subnet =
            SubnetId::from_operators(&operators, SUBNET_COUNT_NZ).expect("valid operators");

        // Should hash operator 42 and return hash % 128
        // Since we have only one operator, it's automatically the minimum
        // SHA256(0x2a00000000000000) mod 128
        assert!((*subnet) < 128);
    }

    #[test]
    fn test_from_operators_order_independence() {
        // MinHash should give same result regardless of operator order
        let ops1 = vec![OperatorId(1), OperatorId(2), OperatorId(3)];
        let ops2 = vec![OperatorId(3), OperatorId(1), OperatorId(2)];
        let ops3 = vec![OperatorId(2), OperatorId(3), OperatorId(1)];

        let subnet1 = SubnetId::from_operators(&ops1, SUBNET_COUNT_NZ).expect("valid operators");
        let subnet2 = SubnetId::from_operators(&ops2, SUBNET_COUNT_NZ).expect("valid operators");
        let subnet3 = SubnetId::from_operators(&ops3, SUBNET_COUNT_NZ).expect("valid operators");

        assert_eq!(subnet1, subnet2);
        assert_eq!(subnet2, subnet3);
    }

    #[test]
    fn test_from_operators_different_sets() {
        // Different operator sets should produce different subnets
        let ops1 = vec![OperatorId(1), OperatorId(2), OperatorId(3)];
        let ops2 = vec![OperatorId(4), OperatorId(5), OperatorId(6)];

        let subnet1 = SubnetId::from_operators(&ops1, SUBNET_COUNT_NZ).expect("valid operators");
        let subnet2 = SubnetId::from_operators(&ops2, SUBNET_COUNT_NZ).expect("valid operators");

        // Different sets should produce different subnets (collision possible but extremely
        // unlikely)
        assert_ne!(subnet1, subnet2);
    }

    #[test]
    fn test_from_operators_same_set_same_subnet() {
        // Same operator set should always give the same subnet
        let operators = vec![
            OperatorId(10),
            OperatorId(20),
            OperatorId(30),
            OperatorId(40),
        ];

        let subnet1 =
            SubnetId::from_operators(&operators, SUBNET_COUNT_NZ).expect("valid operators");
        let subnet2 =
            SubnetId::from_operators(&operators, SUBNET_COUNT_NZ).expect("valid operators");

        assert_eq!(subnet1, subnet2);
    }

    #[test]
    fn test_from_committee_alan_unchanged() {
        // Verify old algorithm still works correctly
        let committee_id = CommitteeId::from([0x01u8; 32]);
        let subnet = SubnetId::from_committee_alan(committee_id, 128);

        // committee_id % 128 should give predictable result
        let expected = U256::from_be_bytes([0x01u8; 32]) % U256::from(128);
        assert_eq!(*subnet, u64::try_from(expected).unwrap());
    }

    #[test]
    fn test_from_committee_alan_various_inputs() {
        // Test several committee IDs to ensure consistent behavior
        let committee_ids = vec![
            CommitteeId::from([0x00u8; 32]),
            CommitteeId::from([0xffu8; 32]),
            CommitteeId::from({
                let mut bytes = [0u8; 32];
                bytes[31] = 42;
                bytes
            }),
        ];

        for committee_id in committee_ids {
            let subnet = SubnetId::from_committee_alan(committee_id, 128);
            assert!((*subnet) < 128);
        }
    }

    #[test]
    fn test_subnet_bounds() {
        // Ensure both algorithms always return subnets within bounds
        let operators = vec![
            OperatorId(u64::MAX),
            OperatorId(u64::MIN),
            OperatorId(12345),
        ];

        let subnet_new =
            SubnetId::from_operators(&operators, SUBNET_COUNT_NZ).expect("valid operators");
        assert!((*subnet_new) < 128);

        let committee_id = CommitteeId::from([0xffu8; 32]);
        let subnet_old = SubnetId::from_committee_alan(committee_id, 128);
        assert!((*subnet_old) < 128);
    }
}
