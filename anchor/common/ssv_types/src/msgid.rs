use std::fmt::{Debug, Formatter};

use bls::PublicKeyBytes;
use derive_more::{Display, From, Into};
use ssz::{Decode, DecodeError, Encode};
use ssz_types::VariableList;
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use typenum::U56;

use crate::{committee::CommitteeId, domain_type::DomainType};

const MESSAGE_ID_LEN: usize = 56;

#[derive(Debug, Display, Copy, Clone, Hash, Eq, PartialEq)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub enum Role {
    Committee,
    Aggregator,
    Proposer,
    SyncCommittee,
    ValidatorRegistration,
    VoluntaryExit,
    AggregatorCommittee,
    PTCCommittee,
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
            Role::PTCCommittee => [7, 0, 0, 0],
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
            [7, 0, 0, 0] => Ok(Role::PTCCommittee),
            _ => Err(DecodeError::NoMatchingVariant),
        }
    }
}

impl Role {
    /// Returns true if this role is a committee-based role (Committee, AggregatorCommittee, or
    /// PTCCommittee).
    ///
    /// Committee roles handle multiple validators in batched operations and have relaxed
    /// validation rules compared to per-validator roles:
    /// - Skip validator index validation (operators may have different validator sets)
    /// - Skip slot advancement checks (allow processing "older" slots within 34-slot window)
    /// - Have different message count limits and validator index occurrence limits
    pub fn is_committee_role(self) -> bool {
        matches!(
            self,
            Role::Committee | Role::AggregatorCommittee | Role::PTCCommittee
        )
    }

    pub fn max_round(self) -> Option<u64> {
        // as per https://github.com/ssvlabs/ssv/blob/6382d4b52ea5e0efd9378a5a00ef481f39d6234f/message/validation/consensus_validation.go#L370
        match self {
            Role::Committee | Role::Aggregator | Role::AggregatorCommittee => Some(12),
            Role::Proposer | Role::SyncCommittee => Some(6),
            // PTC duty starts at 75% slot with ~3s remaining; QUICK_TIMEOUT = 2s
            // gives one useful round within the slot (round 1 expires at +2s).
            // Rounds 2-4 (cumulative +4/+6/+8s) recover from operator start delay
            // or round-1 message loss; rounds 5+ exceed any realistic inclusion
            // window and are dead weight.
            Role::PTCCommittee => Some(4),
            // These roles don't use QBFT consensus
            Role::ValidatorRegistration | Role::VoluntaryExit => None,
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
            Role::Committee | Role::AggregatorCommittee | Role::PTCCommittee => {
                self.0[24..].try_into().ok().map(DutyExecutor::Committee)
            }
            Role::Aggregator
            | Role::Proposer
            | Role::SyncCommittee
            | Role::ValidatorRegistration
            | Role::VoluntaryExit => PublicKeyBytes::deserialize(&self.0[8..])
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
        // SAFETY: This conversion is mathematically infallible.
        // - MessageId is defined as `[u8; 56]` (see MESSAGE_ID_LEN = 56)
        // - VariableList<u8, U56> has max capacity of 56 bytes (U56::USIZE = 56)
        // - Therefore, value.0.to_vec() always produces exactly 56 bytes, which fits within the
        //   VariableList's capacity.
        // The From trait requires infallible conversion; TryFrom would be used
        // if this could fail, but the type system guarantees success here.
        VariableList::new(value.0.to_vec()).expect("infallible: MessageId is [u8; 56] and U56 = 56")
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
    use strum::IntoEnumIterator;

    use super::*;
    use crate::OperatorId;

    // ═══════════════════════════════════════════════════════════════════════════════
    // Role Encoding Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn role_decoding_invalid_variant() {
        assert!(Role::try_from([255, 0, 0, 0].as_slice()).is_err());
        assert!(Role::try_from([0, 1, 0, 0].as_slice()).is_err());
    }

    #[test]
    fn role_roundtrip_all_variants() {
        // Uses EnumIter to automatically test all variants - no manual array needed
        for role in Role::iter() {
            let encoded: [u8; 4] = role.into();
            let decoded = Role::try_from(encoded.as_slice()).unwrap();
            assert_eq!(decoded, role, "Role {:?} failed roundtrip", role);
        }
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
    // AggregatorCommittee Specific Tests
    // ═══════════════════════════════════════════════════════════════════════════════

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

    #[test]
    fn ptc_uses_committee_duty_executor() {
        // PTCCommittee must use Committee-style duty executor (CommitteeId),
        // not Validator-style (PublicKeyBytes)
        let domain = DomainType([0, 0, 0, 0]);
        let committee_id = CommitteeId::from(vec![OperatorId(100), OperatorId(200)]);
        let duty_executor = DutyExecutor::Committee(committee_id);

        let msg_id = MessageId::new(&domain, Role::PTCCommittee, &duty_executor);

        assert_eq!(msg_id.role(), Some(Role::PTCCommittee));

        match msg_id.duty_executor() {
            Some(DutyExecutor::Committee(id)) => assert_eq!(id, committee_id),
            Some(DutyExecutor::Validator(_)) => {
                panic!("PTCCommittee should use Committee duty executor, not Validator")
            }
            None => panic!("Failed to extract duty executor"),
        }
    }

    #[test]
    fn ptc_max_round_is_four() {
        // PTC duty starts at 75% slot; Some(4) is the deliberate cap distinct
        // from Committee's Some(12). Regressions copying the Committee value
        // would compile silently — this guards against that.
        assert_eq!(Role::PTCCommittee.max_round(), Some(4));
    }

    #[test]
    fn ptc_is_committee_role() {
        // is_committee_role drives the validator-index-mismatch skip and
        // slot-advancement skip in message_validator. PTC must qualify.
        assert!(Role::PTCCommittee.is_committee_role());
    }
}
