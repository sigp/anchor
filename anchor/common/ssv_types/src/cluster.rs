use crate::committee::CommitteeId;
use crate::OperatorId;
use derive_more::{Deref, From};
use indexmap::IndexSet;
use ssz_derive::{Decode, Encode};
use std::fmt::Debug;
use types::{Address, Graffiti, PublicKeyBytes};

/// Unique identifier for a cluster
#[derive(Clone, Copy, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct ClusterId(pub [u8; 32]);

impl Debug for ClusterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

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

    pub fn committee_id(&self) -> CommitteeId {
        self.cluster_members
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .into()
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref, Encode, Decode)]
#[ssz(struct_behaviour = "transparent")]
pub struct ValidatorIndex(pub usize);

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
