use std::fmt::Debug;

use derive_more::{Deref, From};
use indexmap::IndexSet;
use multi_index_map::MultiIndexMap;
use ssz_derive::{Decode, Encode};
use types::{Address, Graffiti, PublicKeyBytes};

use crate::{OperatorId, committee::CommitteeId};

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
#[derive(Debug, Clone, PartialEq, Eq, MultiIndexMap)]
pub struct Cluster {
    /// Unique identifier for a Cluster
    #[multi_index(hashed_unique)]
    pub cluster_id: ClusterId,
    /// The owner of the cluster and all of the validators
    #[multi_index(hashed_non_unique)]
    pub owner: Address,
    /// The Eth1 fee address for all validators in the cluster
    pub fee_recipient: Address,
    /// If the Cluster is liquidated or active
    pub liquidated: bool,
    /// Operators in this cluster
    pub cluster_members: IndexSet<OperatorId>,
    #[multi_index(hashed_non_unique)]
    pub committee_id: CommitteeId,
}

impl Cluster {
    /// Create a new cluster
    pub fn new(
        cluster_id: ClusterId,
        owner: Address,
        fee_recipient: Address,
        liquidated: bool,
        cluster_members: IndexSet<OperatorId>,
    ) -> Self {
        let committee_id = cluster_members.iter().cloned().collect::<Vec<_>>().into();
        Self {
            cluster_id,
            owner,
            fee_recipient,
            liquidated,
            cluster_members: cluster_members.clone(),
            committee_id,
        }
    }

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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref, Encode, Decode)]
#[ssz(struct_behaviour = "transparent")]
pub struct ValidatorIndex(pub usize);

impl From<ValidatorIndex> for u64 {
    fn from(value: ValidatorIndex) -> Self {
        value.0 as u64
    }
}

/// General Metadata about a Validator
#[derive(Debug, Clone, multi_index_map::MultiIndexMap)]
pub struct ValidatorMetadata {
    /// Public key of the validator
    #[multi_index(hashed_unique)]
    pub public_key: PublicKeyBytes,
    /// The cluster that is responsible for this validator
    #[multi_index(hashed_non_unique)]
    pub cluster_id: ClusterId,
    /// Index of the validator
    pub index: Option<ValidatorIndex>,
    /// Graffiti
    pub graffiti: Graffiti,
    /// Owner address - computed field from cluster
    pub owner: Address,
    /// Committee ID - computed field from cluster members
    #[multi_index(hashed_non_unique)]
    pub committee_id: CommitteeId,
}
