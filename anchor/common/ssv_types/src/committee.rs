use crate::OperatorId;
use alloy::primitives::keccak256;
use derive_more::{Deref, From};

const COMMITTEE_ID_LEN: usize = 32;

/// Unique identifier for a committee
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct CommitteeId(pub [u8; COMMITTEE_ID_LEN]);

impl From<Vec<OperatorId>> for CommitteeId {
    fn from(mut operator_ids: Vec<OperatorId>) -> Self {
        // Sort the operator IDs
        operator_ids.sort();
        let mut data: Vec<u8> = Vec::with_capacity(operator_ids.len() * 4);

        // Add the operator IDs as 32 byte values
        for id in operator_ids {
            data.extend_from_slice(&id.to_le_bytes());
        }

        // Hash it all
        keccak256(data).0.into()
    }
}

impl TryFrom<&[u8]> for CommitteeId {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, ()> {
        value.try_into().map(CommitteeId).map_err(|_| ())
    }
}
