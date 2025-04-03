use proptest::prelude::*;
use ssv_types::{
    consensus::{BeaconVote, QbftData, QbftMessage, QbftMessageType},
    domain_type::DomainType,
    message::{MsgType, SSVMessage, SignedSSVMessage, RSA_SIGNATURE_SIZE},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
    CommitteeId, OperatorId, Slot, ValidatorIndex,
};
use ssz::Encode;
use types::{
    test_utils::{SeedableRng, TestRandom, XorShiftRng},
    Checkpoint, Epoch, Hash256, PublicKeyBytes, Signature,
};

// All strategies for generating random data
// ------------------------------------------

#[allow(clippy::redundant_closure)]
fn epoch_strategy() -> impl Strategy<Value = Epoch> {
    any::<u64>().prop_map(|num| Epoch::new(num))
}

#[allow(clippy::redundant_closure)]
fn slot_strategy() -> impl Strategy<Value = Slot> {
    any::<u64>().prop_map(|num| Slot::new(num))
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

#[allow(clippy::redundant_closure)]
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

pub fn qbft_message_type_strategy() -> impl Strategy<Value = QbftMessageType> {
    prop_oneof![
        Just(QbftMessageType::Proposal),
        Just(QbftMessageType::Prepare),
        Just(QbftMessageType::Commit),
        Just(QbftMessageType::RoundChange),
    ]
}

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

fn partial_signature_messages_strategy() -> impl Strategy<Value = PartialSignatureMessages> {
    (
        partial_signature_kind_strategy(),
        slot_strategy(),
        proptest::collection::vec(partial_signature_message_strategy(), 1),
    )
        .prop_map(|(kind, slot, messages)| PartialSignatureMessages {
            kind,
            slot,
            messages,
        })
}

fn partial_signature_message_strategy() -> impl Strategy<Value = PartialSignatureMessage> {
    (
        prop::array::uniform32(any::<u8>()),
        any::<u64>(),
        any::<usize>(),
    )
        .prop_map(
            |(root_bytes, signer, validator_index)| PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from(root_bytes),
                signer: OperatorId::from(signer),
                validator_index: ValidatorIndex(validator_index),
            },
        )
}

fn qbft_components_strategy(
    committee: Vec<OperatorId>,
    role: Role,
) -> impl Strategy<Value = (QbftMessageType, u64, u64, u64, MessageId)> {
    (
        qbft_message_type_strategy(),
        any::<u64>(), // height
        any::<u64>(), // round
        any::<u64>(), // data_round
        message_id_strategy(committee.clone(), role),
    )
}

fn qbft_message_strategy(
    committee: Vec<OperatorId>,
    role: Role,
    root: Hash256,
) -> impl Strategy<Value = (QbftMessage, MessageId)> {
    qbft_components_strategy(committee, role).prop_map(
        move |(msg_type, height, round, data_round, msg_id)| {
            let msg = QbftMessage {
                qbft_message_type: msg_type,
                height,
                round,
                identifier: (&msg_id).into(),
                root,
                data_round,
                round_change_justification: vec![],
                prepare_justification: vec![],
            };
            (msg, msg_id)
        },
    )
}

fn signatures_strategy(num_sigs: usize) -> impl Strategy<Value = Vec<Vec<u8>>> {
    proptest::collection::vec(
        proptest::collection::vec(any::<u8>(), RSA_SIGNATURE_SIZE),
        num_sigs,
    )
}

pub fn signed_ssv_message_strategy() -> impl Strategy<Value = SignedSSVMessage> {
    (
        committee_strategy(),
        role_strategy(),
        beacon_vote_strategy(),
    )
        .prop_flat_map(|(committee, role, vote)| {
            let committee_clone = committee.clone();
            let vote_clone = vote.clone();

            qbft_message_strategy(committee_clone.clone(), role, vote.hash()).prop_flat_map(
                move |(msg, msg_id)| {
                    let committee_for_sigs = committee_clone.clone();
                    let vote_for_output = vote_clone.clone();
                    let msg_bytes = msg.as_ssz_bytes();
                    let vote_bytes = vote_for_output.as_ssz_bytes();

                    signatures_strategy(committee_for_sigs.len()).prop_map(move |sigs| {
                        let mut sorted_committee = committee_for_sigs.clone();
                        sorted_committee.sort();

                        let ssv_msg = SSVMessage::new(
                            MsgType::SSVConsensusMsgType,
                            msg_id.clone(),
                            msg_bytes.clone(),
                        )
                        .expect("Failed to create SSVMessage");

                        SignedSSVMessage::new(sigs, sorted_committee, ssv_msg, vote_bytes.clone())
                            .expect("Failed to create SignedSSVMessage")
                    })
                },
            )
        })
}

pub fn signed_partial_sig_message_strategy() -> impl Strategy<Value = SignedSSVMessage> {
    (
        committee_strategy(),
        role_strategy(),
        partial_signature_messages_strategy(),
    )
        .prop_flat_map(|(committee, role, messages)| {
            let committee_clone = committee.clone();
            let messages_bytes = messages.as_ssz_bytes();

            message_id_strategy(committee.clone(), role).prop_flat_map(move |message_id| {
                let committee_for_sigs = committee_clone.clone();
                let sorted_committee = {
                    let mut c = committee_for_sigs.clone();
                    c.sort();
                    c
                };

                let ssv_message = SSVMessage::new(
                    MsgType::SSVPartialSignatureMsgType,
                    message_id,
                    messages_bytes.clone(),
                )
                .expect("Failed to create SSVMessage");

                signatures_strategy(committee_for_sigs.len()).prop_map(move |sigs| {
                    SignedSSVMessage::new(
                        sigs,
                        sorted_committee.clone(),
                        ssv_message.clone(),
                        vec![],
                    )
                    .expect("Failed to create SignedSSVMessage")
                })
            })
        })
}

#[cfg(test)]
mod fuzz_validation_tests {
    use super::*;

    proptest! {
        #[test]
        fn fuzz_validate_consensus_msg(signed_msg in signed_ssv_message_strategy()) {
            println!("{:#?}", signed_msg);
        }

        #[test]
        fn fuzz_validate_partial_sig_messages(signed_msg in signed_partial_sig_message_strategy()) {
            println!("{:#?}", signed_msg);
        }


    }
}
