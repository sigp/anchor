use std::fmt::Debug;

use bls::PublicKeyBytes;
use derive_more::{Deref, Display, From};
use indexmap::IndexSet;
use rusqlite::{
    ToSql,
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, Value, ValueRef},
};
use ssz_derive::{Decode, Encode};
use types::{Address, Graffiti};

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
/// Each cluster is owned by a unique EOA and only that Address may perform operators on the
/// Cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        crate::get_f(self.cluster_members.len()) as u64
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
#[derive(
    Clone, Copy, Display, Debug, Default, Eq, PartialEq, Hash, From, Deref, Encode, Decode,
)]
#[ssz(struct_behaviour = "transparent")]
pub struct ValidatorIndex(pub usize);

impl FromSql for ValidatorIndex {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let v = value.as_i64()?;
        let v = usize::try_from(v).map_err(|_| FromSqlError::OutOfRange(v))?;
        Ok(ValidatorIndex(v))
    }
}

impl ToSql for ValidatorIndex {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let v = i64::try_from(self.0)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Ok(ToSqlOutput::Owned(Value::Integer(v)))
    }
}

impl From<ValidatorIndex> for u64 {
    fn from(value: ValidatorIndex) -> Self {
        value.0 as u64
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
    pub index: Option<ValidatorIndex>,
    /// Graffiti
    pub graffiti: Graffiti,
}
