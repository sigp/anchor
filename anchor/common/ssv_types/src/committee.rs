use std::fmt::{Debug, Formatter};

use derive_more::{Deref, From};
use indexmap::IndexSet;
use sha2::{Digest, Sha256};

use crate::{OperatorId, ValidatorIndex};

const COMMITTEE_ID_LEN: usize = 32;

/// Structure to hold committee members and validator indices
#[derive(Debug, Clone)]
pub struct CommitteeInfo {
    pub committee_members: IndexSet<OperatorId>,
    pub validator_indices: Vec<ValidatorIndex>,
}

impl CommitteeInfo {
    /// Create a mock committee for fuzzing with the specified number of operators
    pub fn new_mock(committee_size: usize) -> Self {
        let mut committee_members = IndexSet::new();
        let mut validator_indices = Vec::new();

        for i in 0..committee_size {
            committee_members.insert(OperatorId(i as u64 + 1));
            validator_indices.push(ValidatorIndex(i));
        }

        Self {
            committee_members,
            validator_indices,
        }
    }
}

/// Unique identifier for a committee
#[derive(Clone, Copy, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct CommitteeId(pub [u8; COMMITTEE_ID_LEN]);

impl Debug for CommitteeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl From<Vec<OperatorId>> for CommitteeId {
    fn from(mut operator_ids: Vec<OperatorId>) -> Self {
        // Sort the operator IDs
        operator_ids.sort();
        operator_ids.as_slice().into()
    }
}

impl From<&[OperatorId]> for CommitteeId {
    fn from(operator_ids: &[OperatorId]) -> Self {
        let mut hasher = Sha256::new();

        // Add the operator IDs as 32 byte values
        for id in operator_ids {
            hasher.update((id.0 as u32).to_le_bytes());
        }

        // Hash it all
        <[u8; COMMITTEE_ID_LEN]>::from(hasher.finalize()).into()
    }
}

impl TryFrom<&[u8]> for CommitteeId {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, ()> {
        value.try_into().map(CommitteeId).map_err(|_| ())
    }
}
