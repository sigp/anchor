use crate::OperatorId;
use crate::Share;
use derive_more::{Deref, From};

/// Unique identifier for a cluster
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct ClusterId(pub u64);

/// A Cluster is a group of Operators that are acting on behalf of a Validator
#[derive(Debug, Clone)]
pub struct Cluster {
    /// Unique identifier for a Cluster
    pub cluster_id: ClusterId,
    /// All of the members of this Cluster
    pub cluster_members: Vec<ClusterMember>,
    /// The number of faulty operator in the Cluster
    pub faulty: u64,
    /// If the Cluster is liquidated or active
    pub liquidated: bool,
}

/// A member of a Cluster. This is just an Operator that holds onto a share of the Validator key
#[derive(Debug, Clone)]
pub struct ClusterMember {
    /// Unique identifier for the Operator this member represents
    pub operator_id: OperatorId,
    /// Unique identifier for the Cluster this member is a part of
    pub cluster_id: ClusterId,
    /// The Share this member is responsible for
    pub share: Share,
}
