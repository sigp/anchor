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
    PTCAttester,
    ProposerPreferences,
    EnvelopeProposer,
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
            Role::PTCAttester => [7, 0, 0, 0],
            Role::ProposerPreferences => [8, 0, 0, 0],
            Role::EnvelopeProposer => [9, 0, 0, 0],
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
            [7, 0, 0, 0] => Ok(Role::PTCAttester),
            [8, 0, 0, 0] => Ok(Role::ProposerPreferences),
            [9, 0, 0, 0] => Ok(Role::EnvelopeProposer),
            _ => Err(DecodeError::NoMatchingVariant),
        }
    }
}

impl Role {
    /// Returns true if this role is a committee-based role (Committee or AggregatorCommittee).
    ///
    /// Committee roles handle multiple validators in batched operations and have relaxed
    /// validation rules compared to per-validator roles:
    /// - Skip validator index validation (operators may have different validator sets)
    /// - Skip slot advancement checks (allow processing "older" slots within 34-slot window)
    /// - Have different message count limits and validator index occurrence limits
    pub fn is_committee_role(self) -> bool {
        matches!(self, Role::Committee | Role::AggregatorCommittee)
    }

    pub fn max_round(self) -> Option<u64> {
        // Caps both the incoming consensus-message round gate and the local QBFT
        // instance round limit. Values mirror go-ssv's maxRound:
        // https://github.com/ssvlabs/ssv/blob/d2352a3dba3e7b309ef090b7a23f4cac1d9002d1/message/validation/consensus_validation.go#L434-L443
        match self {
            Role::Committee | Role::Aggregator | Role::AggregatorCommittee => Some(12),
            Role::Proposer | Role::EnvelopeProposer => Some(2),
            Role::SyncCommittee => Some(6),
            // These roles don't use QBFT consensus
            Role::ValidatorRegistration
            | Role::VoluntaryExit
            | Role::PTCAttester
            | Role::ProposerPreferences => None,
        }
    }

    /// Returns true if this role runs a QBFT consensus round, i.e. it has a
    /// max QBFT round. The validator-scoped roles that do not
    /// (ValidatorRegistration, VoluntaryExit, PTCAttester) return false.
    pub fn is_qbft_role(self) -> bool {
        self.max_round().is_some()
    }

    /// Returns true if this role's duty is bound to a single validator's proposal slot, so the
    /// proposer-duty assignment for that slot decides whether a message can have a duty at all.
    pub fn is_proposer_scoped(self) -> bool {
        match self {
            Role::Proposer | Role::ProposerPreferences | Role::EnvelopeProposer => true,
            Role::Committee
            | Role::Aggregator
            | Role::SyncCommittee
            | Role::ValidatorRegistration
            | Role::VoluntaryExit
            | Role::PTCAttester
            | Role::AggregatorCommittee => false,
        }
    }

    /// monotonicSlotRole reports whether a role's signer advances through slots one at a time, so a
    /// message for a slot below the signer's max is stale and must be rejected. False for
    /// committee roles (state is slot-keyed across many validators) and for proposer
    /// preferences (a signer holds its whole lookahead of proposal slots at once, so a lower
    /// slot is a concurrent duty, not a stale one — its replay bound is the earliness/lateness
    /// window instead)
    pub fn monotonic_slot_role(self) -> bool {
        !self.is_committee_role() && self != Role::ProposerPreferences
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
            Role::Aggregator
            | Role::Proposer
            | Role::SyncCommittee
            | Role::ValidatorRegistration
            | Role::VoluntaryExit
            | Role::PTCAttester
            | Role::ProposerPreferences
            | Role::EnvelopeProposer => PublicKeyBytes::deserialize(&self.0[8..])
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
    fn ptc_attester_uses_validator_duty_executor() {
        // PTCAttester is validator-scoped: it must use Validator-style duty
        // executor (PublicKeyBytes), not Committee-style (CommitteeId).
        let domain = DomainType([0, 0, 0, 0]);
        let public_key = PublicKeyBytes::empty();
        let duty_executor = DutyExecutor::Validator(public_key);

        let msg_id = MessageId::new(&domain, Role::PTCAttester, &duty_executor);

        assert_eq!(msg_id.role(), Some(Role::PTCAttester));

        match msg_id.duty_executor() {
            Some(DutyExecutor::Validator(pk)) => assert_eq!(pk, public_key),
            Some(DutyExecutor::Committee(_)) => {
                panic!("PTCAttester should use Validator duty executor, not Committee")
            }
            None => panic!("Failed to extract duty executor"),
        }
    }

    #[test]
    fn ptc_attester_is_validator_scoped_non_qbft() {
        // Every behavior flip in the PTCAttester retarget rides on these three
        // classifications (validation bucketing, consensus-message rejection,
        // qbft_manager routing). Re-adding PTCAttester to the committee arm or
        // giving it a max round would compile silently; this pins the values.
        assert!(!Role::PTCAttester.is_committee_role());
        assert_eq!(Role::PTCAttester.max_round(), None);
        assert!(!Role::PTCAttester.is_qbft_role());
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // ProposerPreferences Specific Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn proposer_preferences_uses_validator_duty_executor() {
        // ProposerPreferences is validator-scoped: it must use Validator-style duty
        // executor (PublicKeyBytes), not Committee-style (CommitteeId).
        let domain = DomainType([0, 0, 0, 0]);
        let public_key = PublicKeyBytes::empty();
        let duty_executor = DutyExecutor::Validator(public_key);

        let msg_id = MessageId::new(&domain, Role::ProposerPreferences, &duty_executor);

        assert_eq!(msg_id.role(), Some(Role::ProposerPreferences));

        match msg_id.duty_executor() {
            Some(DutyExecutor::Validator(pk)) => assert_eq!(pk, public_key),
            Some(DutyExecutor::Committee(_)) => {
                panic!("ProposerPreferences should use Validator duty executor, not Committee")
            }
            None => panic!("Failed to extract duty executor"),
        }
    }

    #[test]
    fn proposer_preferences_is_validator_scoped_non_qbft() {
        // Every behavior flip in the ProposerPreferences retarget rides on these
        // three classifications (validation bucketing, consensus-message rejection,
        // qbft_manager routing). Re-adding ProposerPreferences to the committee arm
        // or giving it a max round would compile silently; this pins the values.
        assert!(!Role::ProposerPreferences.is_committee_role());
        assert_eq!(Role::ProposerPreferences.max_round(), None);
        assert!(!Role::ProposerPreferences.is_qbft_role());
    }

    #[test]
    fn role_qbft_classification_is_pinned() {
        // Adding a new Role forces a decision in max_round()'s exhaustive
        // match, but nothing checks the decision is right: a role landing in
        // the wrong arm silently flips its consensus-message and routing
        // behavior. Pin every existing role on both sides of the partition.
        for role in [
            Role::Committee,
            Role::Aggregator,
            Role::AggregatorCommittee,
            Role::Proposer,
            Role::SyncCommittee,
            Role::EnvelopeProposer,
        ] {
            assert!(role.is_qbft_role(), "{role:?} runs QBFT");
            assert!(role.max_round().is_some(), "{role:?} must have a max round");
        }
        for role in [
            Role::ValidatorRegistration,
            Role::VoluntaryExit,
            Role::PTCAttester,
            Role::ProposerPreferences,
        ] {
            assert!(!role.is_qbft_role(), "{role:?} must not run QBFT");
            assert!(
                role.max_round().is_none(),
                "{role:?} must not have a max round"
            );
        }
    }

    #[test]
    fn role_monotonic_slot_classification_is_pinned() {
        // `monotonic_slot_role()` gates the stale-slot rejection in partial-signature
        // validation. A role landing in the wrong partition silently flips whether an
        // earlier-slot message is rejected as advanced or accepted as concurrent, so pin
        // every role on both sides. Non-monotonic: committee roles (state is slot-keyed
        // across many validators) and ProposerPreferences (a signer holds its whole
        // lookahead of proposal slots at once). Monotonic: the remaining six.
        for role in [
            Role::Committee,
            Role::AggregatorCommittee,
            Role::ProposerPreferences,
        ] {
            assert!(
                !role.monotonic_slot_role(),
                "{role:?} must NOT be a monotonic-slot role"
            );
        }
        for role in [
            Role::Aggregator,
            Role::Proposer,
            Role::SyncCommittee,
            Role::ValidatorRegistration,
            Role::VoluntaryExit,
            Role::PTCAttester,
            Role::EnvelopeProposer,
        ] {
            assert!(
                role.monotonic_slot_role(),
                "{role:?} must be a monotonic-slot role"
            );
        }
    }

    /// Tests that EnvelopeProposer is a validator-scoped QBFT role with
    /// round cut-off 2.
    #[test]
    fn envelope_proposer_is_validator_scoped_qbft_with_round_cutoff_two() {
        assert!(
            !Role::EnvelopeProposer.is_committee_role(),
            "EnvelopeProposer is per-validator, not a committee role"
        );
        assert_eq!(
            Role::EnvelopeProposer.max_round(),
            Some(2),
            "Envelope QBFT cut-off round must be 2"
        );
        assert!(
            Role::EnvelopeProposer.is_qbft_role(),
            "EnvelopeProposer runs QBFT (max_round is Some)"
        );
        assert!(
            Role::EnvelopeProposer.monotonic_slot_role(),
            "EnvelopeProposer signers advance slot-by-slot; lower slots are stale"
        );

        // Wire byte 9 for EnvelopeProposer check.
        let bytes: [u8; 4] = Role::EnvelopeProposer.into();
        assert_eq!(bytes, [9, 0, 0, 0], "EnvelopeProposer wire byte is 9");
        assert_eq!(
            Role::try_from(bytes.as_slice()).unwrap(),
            Role::EnvelopeProposer,
            "wire byte 9 decodes back to EnvelopeProposer"
        );

        // duty_executor resolves to Validator (per-proposer, pubkey-scoped).
        let domain = DomainType([0, 0, 0, 1]);
        let pk = PublicKeyBytes::empty();
        let msg_id = MessageId::new(
            &domain,
            Role::EnvelopeProposer,
            &DutyExecutor::Validator(pk),
        );
        assert_eq!(
            msg_id.duty_executor(),
            Some(DutyExecutor::Validator(pk)),
            "EnvelopeProposer resolves to a validator-scoped duty executor"
        );
    }

    /// Pins the proposer QBFT and sync-committee round caps.
    #[test]
    fn proposer_and_sync_committee_max_rounds_are_pinned() {
        assert_eq!(
            Role::Proposer.max_round(),
            Some(2),
            "`Role::Proposer` must cap QBFT at round 2"
        );
        assert_eq!(
            Role::SyncCommittee.max_round(),
            Some(6),
            "`Role::SyncCommittee` must maintain its round 6 cap"
        );
    }
}
