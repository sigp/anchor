//! Core subnet types and calculations.
//!
//! This module contains the fundamental types for subnet identification and
//! calculation algorithms used in the SSV network.

use std::{num::NonZeroU64, ops::Deref};

use alloy::primitives::ruint::aliases::U256;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ssv_types::{CommitteeId, OperatorId};

/// Number of subnets in the SSV network.
pub const SUBNET_COUNT: usize = 128;

/// Bit array representing subnet membership.
pub type SubnetBits = [u8; SUBNET_COUNT / 8];

/// Errors that can occur during subnet calculation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubnetCalculationError {
    /// The operator list provided was empty.
    EmptyOperatorList,
    /// The subnet count is invalid (zero).
    InvalidSubnetCount,
    /// The calculated subnet ID doesn't fit in a u64.
    SubnetIdOutOfRange,
}

/// Identifies a subnet in the SSV network.
///
/// Subnets are used to partition the gossipsub network, reducing the number
/// of topics each node must subscribe to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubnetId(#[serde(with = "serde_utils::quoted_u64")] u64);

impl SubnetId {
    /// Create a new SubnetId from a raw u64 value.
    pub fn new(id: u64) -> Self {
        id.into()
    }

    /// Calculate subnet using committee ID (Alan fork algorithm).
    ///
    /// This is the pre-fork algorithm that derives the subnet from the committee ID.
    ///
    /// # Algorithm
    ///
    /// `committee_id % subnet_count`
    pub fn from_committee_alan(committee_id: CommitteeId, subnet_count: usize) -> Self {
        // Derive a numeric "committee ID" and convert to an index in [0..subnet_count].
        let id = U256::from_be_bytes(*committee_id);
        SubnetId(
            (id % U256::from(subnet_count))
                .try_into()
                .expect("modulo must be < subnet_count"),
        )
    }

    /// Calculate subnet using MinHash of operator IDs (post-fork algorithm).
    ///
    /// When an operator participates in multiple different operator sets, MinHash
    /// increases the likelihood those sets map to the same subnet (if that operator
    /// has the minimum hash). This reduces the number of subnets each operator must
    /// monitor.
    ///
    /// # Algorithm
    ///
    /// 1. For each operator ID, encode as little-endian u64 (8 bytes)
    /// 2. SHA256 hash each encoded operator ID individually
    /// 3. Find the minimum hash value
    /// 4. Return `min_hash % subnet_count`
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

/// Events emitted by the subnet service to notify the network layer.
pub enum SubnetEvent {
    /// Join a subnet, optionally with an expected message rate for scoring.
    Join(SubnetId, Option<f64>),
    /// Leave a subnet.
    Leave(SubnetId),
    /// Message rate has changed for an already-joined subnet (only emitted when scoring is
    /// enabled).
    RateUpdate(SubnetId, f64),
}

#[cfg(test)]
mod tests {
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
