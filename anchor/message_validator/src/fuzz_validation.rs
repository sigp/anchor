use proptest::prelude::*;
use ssv_types::consensus::{BeaconVote, QbftData, QbftMessage, QbftMessageType};
use ssv_types::domain_type::DomainType;
use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage, RSA_SIGNATURE_SIZE};
use ssv_types::msgid::{DutyExecutor, MessageId, Role};
use ssv_types::partial_sig::{
    PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages,
};
use ssv_types::{CommitteeId, CommitteeInfo, IndexSet, OperatorId, ValidatorIndex};
use ssz::Encode;
use types::test_utils::{SeedableRng, TestRandom, XorShiftRng};
use types::{Checkpoint, Epoch, Hash256, PublicKeyBytes, Signature, Slot};

// All strategies for generating random data
// ------------------------------------------

#[allow(clippy::redundant_closure)]
fn epoch_strategy() -> impl Strategy<Value = Epoch> {
    any::<u64>().prop_map(|num| Epoch::new(num))
}

fn checkpoint_strategy() -> impl Strategy<Value = Checkpoint> {
    (epoch_strategy(), prop::array::uniform32(any::<u8>())).prop_map(|(epoch, bytes)| {
        // Assuming Hash256 can be created from [u8; 32]
        let root = Hash256::from(bytes);
        Checkpoint { epoch, root }
    })
}

fn beacon_vote_strategy() -> impl Strategy<Value = BeaconVote> {
    (
        prop::array::uniform32(any::<u8>()),
        checkpoint_strategy(),
        checkpoint_strategy(),
    )
        .prop_map(|(root, cp1, cp2)| BeaconVote {
            block_root: Hash256::from(root),
            source: cp1,
            target: cp2,
        })
}

fn domain_strategy() -> impl Strategy<Value = DomainType> {
    prop::array::uniform4(prop::num::u8::ANY).prop_map(DomainType::from)
}

fn committee_strategy() -> impl Strategy<Value = Vec<OperatorId>> {
    prop_oneof![Just(4usize), Just(7usize), Just(10usize), Just(13usize)].prop_flat_map(|size| {
        prop::collection::vec(any::<u64>(), size..=size).prop_map(|nums| {
            nums.into_iter()
                .map(|num| OperatorId::from(num))
                .collect::<Vec<_>>()
        })
    })
}

fn public_key_strategy() -> impl Strategy<Value = PublicKeyBytes> {
    prop::array::uniform::<_, 16>(prop::num::u8::ANY).prop_map(|seed| {
        let rng = &mut XorShiftRng::from_seed(seed);
        PublicKeyBytes::random_for_test(rng)
    })
}

fn duty_executor_strategy(operators: Vec<OperatorId>) -> impl Strategy<Value = DutyExecutor> {
    prop_oneof![
        Just(DutyExecutor::Committee(CommitteeId::from(operators))),
        public_key_strategy().prop_map(DutyExecutor::Validator)
    ]
}

fn message_id_strategy(operators: Vec<OperatorId>, role: Role) -> impl Strategy<Value = MessageId> {
    (domain_strategy(), duty_executor_strategy(operators))
        .prop_flat_map(move |(domain, executor)| Just(MessageId::new(&domain, role, &executor)))
}

// Strategy for generating QbftMessageType
pub fn qbft_message_type_strategy() -> impl Strategy<Value = QbftMessageType> {
    prop_oneof![
        Just(QbftMessageType::Proposal),
        Just(QbftMessageType::Prepare),
        Just(QbftMessageType::Commit),
        Just(QbftMessageType::RoundChange),
    ]
}

// Strategy for generating PartialSignatureKind
pub fn partial_signature_kind_strategy() -> impl Strategy<Value = PartialSignatureKind> {
    prop_oneof![
        Just(PartialSignatureKind::PostConsensus),
        Just(PartialSignatureKind::RandaoPartialSig),
        Just(PartialSignatureKind::SelectionProofPartialSig),
        Just(PartialSignatureKind::ContributionProofs),
        Just(PartialSignatureKind::ValidatorRegistration),
        Just(PartialSignatureKind::VoluntaryExit),
    ]
}

// Strategy for generating Role
pub fn role_strategy() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::Committee),
        Just(Role::Proposer),
        Just(Role::Aggregator),
        Just(Role::SyncCommittee),
        Just(Role::ValidatorRegistration),
        Just(Role::VoluntaryExit),
    ]
}

#[cfg(test)]
mod fuzz_validation_tests {
    use super::*;

    proptest! {

        #[test]
        fn test_message_building(
            (
                committee,
                msg_id,
                vote,
                msg_type,
                height,
                round,
                data_round
            ) in (committee_strategy(), role_strategy()).prop_flat_map(|(committee, role)| {
                (
                    Just(committee.clone()),
                    message_id_strategy(committee, role),
                    beacon_vote_strategy(),
                    qbft_message_type_strategy(),
                    any::<u64>(),
                    any::<u64>(),
                    any::<u64>()
                )
            })
        ) {
            let msg = QbftMessage {
                qbft_message_type: msg_type,
                height,
                round,
                identifier: (&msg_id).into(),
                root: vote.hash(),
                data_round,
                round_change_justification: vec![],
                prepare_justification: vec![]
            };
            let full_data = vote.as_ssz_bytes();

            let ssv_message = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id.clone(), msg.as_ssz_bytes()).unwrap();
            let _signed_ssv_message = SignedSSVMessage::new(
                vec![],
                committee,
                ssv_message,
                full_data
            ).unwrap();
        }

    }
}
