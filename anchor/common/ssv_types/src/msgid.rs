use std::fmt::{Debug, Formatter};

use derive_more::{Display, From, Into};
use ssz::{Decode, DecodeError, Encode};
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use types::{PublicKeyBytes, VariableList, typenum::U56};

use crate::{committee::CommitteeId, domain_type::DomainType};

const MESSAGE_ID_LEN: usize = 56;

#[derive(Debug, Display, Copy, Clone, Hash, Eq, PartialEq)]
pub enum Role {
    Committee,
    Aggregator,
    Proposer,
    SyncCommittee,
    ValidatorRegistration,
    VoluntaryExit,
    AggregatorCommittee,
}

impl From<Role> for [u8; 4] {
    fn from(value: Role) -> Self {
        match value {
            Role::Committee => [0, 0, 0, 0],
            Role::Aggregator => [1, 0, 0, 0],
            Role::Proposer => [2, 0, 0, 0],
            Role::SyncCommittee => [3, 0, 0, 0],
            Role::ValidatorRegistration => [4, 0, 0, 0],
            Role::VoluntaryExit => [5, 0, 0, 0],
            Role::AggregatorCommittee => [6, 0, 0, 0],
        }
    }
}

impl TryFrom<&[u8]> for Role {
    type Error = DecodeError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value {
            [0, 0, 0, 0] => Ok(Role::Committee),
            [1, 0, 0, 0] => Ok(Role::Aggregator),
            [2, 0, 0, 0] => Ok(Role::Proposer),
            [3, 0, 0, 0] => Ok(Role::SyncCommittee),
            [4, 0, 0, 0] => Ok(Role::ValidatorRegistration),
            [5, 0, 0, 0] => Ok(Role::VoluntaryExit),
            [6, 0, 0, 0] => Ok(Role::AggregatorCommittee),
            _ => Err(DecodeError::NoMatchingVariant),
        }
    }
}

impl Role {
    pub fn max_round(self) -> Option<u64> {
        // as per https://github.com/ssvlabs/ssv/blob/6382d4b52ea5e0efd9378a5a00ef481f39d6234f/message/validation/consensus_validation.go#L370
        match self {
            Role::Committee | Role::Aggregator | Role::AggregatorCommittee => Some(12),
            Role::Proposer | Role::SyncCommittee => Some(6),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum DutyExecutor {
    Committee(CommitteeId),
    Validator(PublicKeyBytes),
}

#[derive(Clone, Hash, Eq, PartialEq, From, Into)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct MessageId([u8; 56]);

impl TreeHash for MessageId {
    fn tree_hash_type() -> TreeHashType {
        TreeHashType::Vector
    }

    fn tree_hash_packed_encoding(&self) -> PackedEncoding {
        unreachable!("Vector should never be packed.")
    }

    fn tree_hash_packing_factor() -> usize {
        unreachable!("Vector should never be packed.")
    }

    fn tree_hash_root(&self) -> tree_hash::Hash256 {
        self.0.tree_hash_root()
    }
}

impl Debug for MessageId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl MessageId {
    pub fn new(domain: &DomainType, role: Role, duty_executor: &DutyExecutor) -> Self {
        let mut id = [0; 56];
        id[0..4].copy_from_slice(&domain.0);
        id[4..8].copy_from_slice(&<[u8; 4]>::from(role));
        match duty_executor {
            DutyExecutor::Committee(committee_id) => {
                id[24..].copy_from_slice(committee_id.as_slice())
            }
            DutyExecutor::Validator(public_key) => {
                id[8..].copy_from_slice(public_key.as_serialized())
            }
        }

        MessageId(id)
    }

    pub fn domain(&self) -> DomainType {
        DomainType(
            self.0[0..4]
                .try_into()
                .expect("we know the slice has the correct length"),
        )
    }

    pub fn role(&self) -> Option<Role> {
        self.0[4..8].try_into().ok()
    }

    pub fn duty_executor(&self) -> Option<DutyExecutor> {
        // which kind of executor we need to get depends on the role
        match self.role()? {
            Role::Committee | Role::AggregatorCommittee => {
                self.0[24..].try_into().ok().map(DutyExecutor::Committee)
            }
            _ => PublicKeyBytes::deserialize(&self.0[8..])
                .ok()
                .map(DutyExecutor::Validator),
        }
    }
}

impl AsRef<[u8]> for MessageId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl TryFrom<&[u8]> for MessageId {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, ()> {
        value.try_into().map(MessageId).map_err(|_| ())
    }
}

impl From<&MessageId> for VariableList<u8, U56> {
    fn from(value: &MessageId) -> Self {
        value.0.to_vec().into()
    }
}

impl Encode for MessageId {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.0);
    }

    fn ssz_fixed_len() -> usize {
        MESSAGE_ID_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        MESSAGE_ID_LEN
    }
}

impl Decode for MessageId {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        MESSAGE_ID_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != MESSAGE_ID_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: MESSAGE_ID_LEN,
            });
        }
        let mut id = [0u8; MESSAGE_ID_LEN];
        id.copy_from_slice(bytes);
        Ok(MessageId(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperatorId, committee::CommitteeId, domain_type::DomainType};

    #[test]
    fn test_aggregator_committee_role_serialization_roundtrip() {
        // Test that Role::AggregatorCommittee encodes to [6, 0, 0, 0]
        let role = Role::AggregatorCommittee;
        let encoded: [u8; 4] = role.into();
        assert_eq!(
            encoded,
            [6, 0, 0, 0],
            "AggregatorCommittee should encode to [6, 0, 0, 0]"
        );

        // Test that [6, 0, 0, 0] decodes back to Role::AggregatorCommittee
        let decoded = Role::try_from(&encoded[..]).expect("Should decode successfully");
        assert_eq!(
            decoded,
            Role::AggregatorCommittee,
            "Should decode back to AggregatorCommittee"
        );
    }

    #[test]
    fn test_message_id_construction_with_aggregator_committee() {
        // Create a test domain
        let domain = DomainType([1, 2, 3, 4]);

        // Create a test committee ID
        let operator_ids = vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
        let committee_id: CommitteeId = operator_ids.as_slice().into();

        // Create a MessageId with AggregatorCommittee role and Committee executor
        let msg_id = MessageId::new(
            &domain,
            Role::AggregatorCommittee,
            &DutyExecutor::Committee(committee_id),
        );

        // Verify the role is at bytes 4-7
        assert_eq!(
            &msg_id.0[4..8],
            &[6, 0, 0, 0],
            "Role should be encoded at bytes 4-7"
        );

        // Verify the committee ID is at bytes 24-55 (last 32 bytes)
        assert_eq!(
            &msg_id.0[24..56],
            committee_id.as_slice(),
            "CommitteeId should be at bytes 24-55"
        );

        // Verify bytes 8-23 are zeros (not used for committee routing)
        assert_eq!(
            &msg_id.0[8..24],
            &[0u8; 16],
            "Bytes 8-23 should be zeros for committee routing"
        );

        // Verify we can extract the role and duty executor back
        assert_eq!(msg_id.role(), Some(Role::AggregatorCommittee));
        assert_eq!(
            msg_id.duty_executor(),
            Some(DutyExecutor::Committee(committee_id))
        );
    }

    #[test]
    fn test_aggregator_committee_max_round() {
        // Test that Role::AggregatorCommittee.max_round() returns Some(12)
        assert_eq!(
            Role::AggregatorCommittee.max_round(),
            Some(12),
            "AggregatorCommittee max_round should be Some(12)"
        );

        // Also verify other roles for consistency
        assert_eq!(Role::Committee.max_round(), Some(12));
        assert_eq!(Role::Aggregator.max_round(), Some(12));
        assert_eq!(Role::Proposer.max_round(), Some(6));
        assert_eq!(Role::SyncCommittee.max_round(), Some(6));
        assert_eq!(Role::ValidatorRegistration.max_round(), None);
        assert_eq!(Role::VoluntaryExit.max_round(), None);
    }
}
