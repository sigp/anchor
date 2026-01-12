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
    use crate::OperatorId;

    // ═══════════════════════════════════════════════════════════════════════════════
    // Role Encoding Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn role_encoding_all_variants() {
        assert_eq!(<[u8; 4]>::from(Role::Committee), [0, 0, 0, 0]);
        assert_eq!(<[u8; 4]>::from(Role::Aggregator), [1, 0, 0, 0]);
        assert_eq!(<[u8; 4]>::from(Role::Proposer), [2, 0, 0, 0]);
        assert_eq!(<[u8; 4]>::from(Role::SyncCommittee), [3, 0, 0, 0]);
        assert_eq!(<[u8; 4]>::from(Role::ValidatorRegistration), [4, 0, 0, 0]);
        assert_eq!(<[u8; 4]>::from(Role::VoluntaryExit), [5, 0, 0, 0]);
        assert_eq!(<[u8; 4]>::from(Role::AggregatorCommittee), [6, 0, 0, 0]);
    }

    #[test]
    fn role_decoding_all_variants() {
        assert_eq!(
            Role::try_from([0, 0, 0, 0].as_slice()).unwrap(),
            Role::Committee
        );
        assert_eq!(
            Role::try_from([1, 0, 0, 0].as_slice()).unwrap(),
            Role::Aggregator
        );
        assert_eq!(
            Role::try_from([2, 0, 0, 0].as_slice()).unwrap(),
            Role::Proposer
        );
        assert_eq!(
            Role::try_from([3, 0, 0, 0].as_slice()).unwrap(),
            Role::SyncCommittee
        );
        assert_eq!(
            Role::try_from([4, 0, 0, 0].as_slice()).unwrap(),
            Role::ValidatorRegistration
        );
        assert_eq!(
            Role::try_from([5, 0, 0, 0].as_slice()).unwrap(),
            Role::VoluntaryExit
        );
        assert_eq!(
            Role::try_from([6, 0, 0, 0].as_slice()).unwrap(),
            Role::AggregatorCommittee
        );
    }

    #[test]
    fn role_decoding_invalid_variant() {
        assert!(Role::try_from([7, 0, 0, 0].as_slice()).is_err());
        assert!(Role::try_from([255, 0, 0, 0].as_slice()).is_err());
        assert!(Role::try_from([0, 1, 0, 0].as_slice()).is_err());
    }

    #[test]
    fn role_roundtrip_all_variants() {
        let roles = [
            Role::Committee,
            Role::Aggregator,
            Role::Proposer,
            Role::SyncCommittee,
            Role::ValidatorRegistration,
            Role::VoluntaryExit,
            Role::AggregatorCommittee,
        ];

        for role in roles {
            let encoded: [u8; 4] = role.into();
            let decoded = Role::try_from(encoded.as_slice()).unwrap();
            assert_eq!(decoded, role, "Role {:?} failed roundtrip", role);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // Role::max_round() Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn role_max_round_consensus_roles() {
        // Consensus roles that use 12 rounds
        assert_eq!(Role::Committee.max_round(), Some(12));
        assert_eq!(Role::Aggregator.max_round(), Some(12));
        assert_eq!(Role::AggregatorCommittee.max_round(), Some(12));
    }

    #[test]
    fn role_max_round_shorter_roles() {
        // Roles that use 6 rounds
        assert_eq!(Role::Proposer.max_round(), Some(6));
        assert_eq!(Role::SyncCommittee.max_round(), Some(6));
    }

    #[test]
    fn role_max_round_non_consensus_roles() {
        // Non-consensus roles have no max round
        assert_eq!(Role::ValidatorRegistration.max_round(), None);
        assert_eq!(Role::VoluntaryExit.max_round(), None);
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // MessageId Construction Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn message_id_new_committee_role() {
        let domain = DomainType([0xAA, 0xBB, 0xCC, 0xDD]);
        let committee_id = CommitteeId::from(vec![OperatorId(1), OperatorId(2), OperatorId(3)]);
        let duty_executor = DutyExecutor::Committee(committee_id);

        let msg_id = MessageId::new(&domain, Role::Committee, &duty_executor);

        assert_eq!(msg_id.domain(), domain);
        assert_eq!(msg_id.role(), Some(Role::Committee));
        assert_eq!(msg_id.duty_executor(), Some(duty_executor));
    }

    #[test]
    fn message_id_new_aggregator_committee_role() {
        // AggregatorCommittee should use Committee-style duty executor (committee ID)
        let domain = DomainType([0x11, 0x22, 0x33, 0x44]);
        let committee_id = CommitteeId::from(vec![OperatorId(5), OperatorId(10), OperatorId(15)]);
        let duty_executor = DutyExecutor::Committee(committee_id);

        let msg_id = MessageId::new(&domain, Role::AggregatorCommittee, &duty_executor);

        assert_eq!(msg_id.domain(), domain);
        assert_eq!(msg_id.role(), Some(Role::AggregatorCommittee));
        assert_eq!(msg_id.duty_executor(), Some(duty_executor));
    }

    #[test]
    fn message_id_new_validator_role() {
        let domain = DomainType([0x01, 0x02, 0x03, 0x04]);
        let public_key = PublicKeyBytes::empty();
        let duty_executor = DutyExecutor::Validator(public_key);

        let msg_id = MessageId::new(&domain, Role::Aggregator, &duty_executor);

        assert_eq!(msg_id.domain(), domain);
        assert_eq!(msg_id.role(), Some(Role::Aggregator));
        assert_eq!(msg_id.duty_executor(), Some(duty_executor));
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // MessageId::duty_executor() Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn message_id_duty_executor_committee_roles() {
        // Both Committee and AggregatorCommittee should extract CommitteeId
        let domain = DomainType([0, 0, 0, 0]);
        let committee_id = CommitteeId::from(vec![OperatorId(1), OperatorId(2)]);

        for role in [Role::Committee, Role::AggregatorCommittee] {
            let duty_executor = DutyExecutor::Committee(committee_id);
            let msg_id = MessageId::new(&domain, role, &duty_executor);

            match msg_id.duty_executor() {
                Some(DutyExecutor::Committee(extracted_id)) => {
                    assert_eq!(
                        extracted_id, committee_id,
                        "Role {:?} failed committee ID extraction",
                        role
                    );
                }
                other => panic!(
                    "Expected DutyExecutor::Committee for {:?}, got {:?}",
                    role, other
                ),
            }
        }
    }

    #[test]
    fn message_id_duty_executor_validator_roles() {
        // Validator-based roles should extract PublicKeyBytes
        let domain = DomainType([0, 0, 0, 0]);
        let public_key = PublicKeyBytes::empty();

        let validator_roles = [
            Role::Aggregator,
            Role::Proposer,
            Role::SyncCommittee,
            Role::ValidatorRegistration,
            Role::VoluntaryExit,
        ];

        for role in validator_roles {
            let duty_executor = DutyExecutor::Validator(public_key);
            let msg_id = MessageId::new(&domain, role, &duty_executor);

            match msg_id.duty_executor() {
                Some(DutyExecutor::Validator(extracted_key)) => {
                    assert_eq!(
                        extracted_key, public_key,
                        "Role {:?} failed public key extraction",
                        role
                    );
                }
                other => panic!(
                    "Expected DutyExecutor::Validator for {:?}, got {:?}",
                    role, other
                ),
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // AggregatorCommittee Specific Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn aggregator_committee_role_encoding() {
        // Verify the new variant encodes correctly for wire compatibility with Go SSV
        let encoded: [u8; 4] = Role::AggregatorCommittee.into();
        assert_eq!(
            encoded,
            [6, 0, 0, 0],
            "AggregatorCommittee should encode as [6, 0, 0, 0]"
        );
    }

    #[test]
    fn aggregator_committee_uses_committee_duty_executor() {
        // This is the critical test: AggregatorCommittee must use Committee-style
        // duty executor (CommitteeId), not Validator-style (PublicKeyBytes)
        let domain = DomainType([0, 0, 0, 0]);
        let committee_id = CommitteeId::from(vec![OperatorId(100), OperatorId(200)]);
        let duty_executor = DutyExecutor::Committee(committee_id);

        let msg_id = MessageId::new(&domain, Role::AggregatorCommittee, &duty_executor);

        // Verify the role is correctly stored and retrieved
        assert_eq!(msg_id.role(), Some(Role::AggregatorCommittee));

        // Verify duty_executor() correctly interprets as Committee (not Validator)
        match msg_id.duty_executor() {
            Some(DutyExecutor::Committee(id)) => assert_eq!(id, committee_id),
            Some(DutyExecutor::Validator(_)) => {
                panic!("AggregatorCommittee should use Committee duty executor, not Validator")
            }
            None => panic!("Failed to extract duty executor"),
        }
    }
}
