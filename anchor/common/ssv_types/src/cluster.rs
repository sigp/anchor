use crate::OperatorId;
use derive_more::{Deref, From};
use std::collections::HashSet;
use types::{Address, Graffiti, PublicKey};

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
    /// The number of faulty operator in the Cluster
    pub faulty: u64,
    /// If the Cluster is liquidated or active
    pub liquidated: bool,
    /// Operators in this cluster
    pub cluster_members: HashSet<OperatorId>,
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

/// General Metadata about a Validator
#[derive(Debug, Clone)]
pub struct ValidatorMetadata {
    /// Public key of the validator
    pub public_key: PublicKey,
    /// The cluster that is responsible for this validator
    pub cluster_id: ClusterId,
    /// Index of the validator
    pub index: ValidatorIndex,
    /// Graffiti
    pub graffiti: Graffiti,
}
