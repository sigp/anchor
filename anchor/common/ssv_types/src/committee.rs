use crate::OperatorId;
use derive_more::{Deref, From};
use sha2::{Digest, Sha256};

const COMMITTEE_ID_LEN: usize = 32;

/// Unique identifier for a committee
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct CommitteeId(pub [u8; COMMITTEE_ID_LEN]);

impl From<Vec<OperatorId>> for CommitteeId {
    fn from(mut operator_ids: Vec<OperatorId>) -> Self {
        // Sort the operator IDs
        operator_ids.sort();
        let mut hasher = Sha256::new();

        // Add the operator IDs as 32 byte values
        for id in operator_ids {
            hasher.update((*id as u32).to_le_bytes());
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
