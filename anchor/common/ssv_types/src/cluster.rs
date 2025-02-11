use crate::OperatorId;
use derive_more::{Deref, From};
use indexmap::IndexSet;
use ssz::{Decode, DecodeError, Encode};
use types::{Address, Graffiti, PublicKeyBytes};

/// Unique identifier for a cluster
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct ClusterId(pub [u8; 32]);

/// A Cluster is a group of Operators that are acting on behalf of one or more Validators
///
/// Each cluster is owned by a unqiue EOA and only that Address may perform operators on the
/// Cluster.
#[derive(Debug, Clone)]
pub struct Cluster {
    /// Unique identifier for a Cluster
    pub cluster_id: ClusterId,
    /// The owner of the cluster and all of the validators
    pub owner: Address,
    /// The Eth1 fee address for all validators in the cluster
    pub fee_recipient: Address,
    /// If the Cluster is liquidated or active
    pub liquidated: bool,
    /// Operators in this cluster
    pub cluster_members: IndexSet<OperatorId>,
}

impl Cluster {
    /// Returns the maximum tolerable number of faulty members.
    ///
    /// In other words, return the largest f where 3f+1 is less than or equal the number of
    /// cluster members.
    ///
    /// Exception: Returns 0 if there are no cluster members
    pub fn get_f(&self) -> u64 {
        (self.cluster_members.len().saturating_sub(1) / 3) as u64
    }
}

/// A member of a Cluster.
/// This is an Operator that holds a piece of the keyshare for each validator in the cluster
#[derive(Debug, Clone)]
pub struct ClusterMember {
    /// Unique identifier for the Operator this member represents
    pub operator_id: OperatorId,
    /// Unique identifier for the Cluster this member is a part of
    pub cluster_id: ClusterId,
}

/// Index of the validator in the validator registry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct ValidatorIndex(pub usize);

impl Encode for ValidatorIndex {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        // Convert usize to u64 for consistent encoding across platforms
        let value = self.0 as u64;
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn ssz_fixed_len() -> usize {
        8 // Size of u64 in bytes
    }

    fn ssz_bytes_len(&self) -> usize {
        8 // Size of u64 in bytes
    }
}

impl Decode for ValidatorIndex {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        8 // Size of u64 in bytes
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != 8 {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: 8,
            });
        }

        let value = u64::from_le_bytes(bytes.try_into().unwrap());
        Ok(ValidatorIndex(value as usize))
    }
}

/// General Metadata about a Validator
#[derive(Debug, Clone)]
pub struct ValidatorMetadata {
    /// Public key of the validator
    pub public_key: PublicKeyBytes,
    /// The cluster that is responsible for this validator
    pub cluster_id: ClusterId,
    /// Index of the validator
    pub index: ValidatorIndex,
    /// Graffiti
    pub graffiti: Graffiti,
}
