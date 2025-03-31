use crate::{Config, ConfigBuilder, DefaultLeaderFunction, InstanceHeight, Round};
use proptest::prelude::*;
use sha2::{Digest, Sha256};
use ssv_types::consensus::{QbftData, QbftMessage, QbftMessageType};
use ssv_types::domain_type::DomainType;
use ssz::{Decode, Encode};
use crate::WrappedQbftMessage;
use ssv_types::msgid::{DutyExecutor, MessageId, Role};
use ssv_types::{CommitteeId, IndexSet, OperatorId};
use ssv_types::message::{SignedSSVMessage, SSVMessage, MsgType, RSA_SIGNATURE_SIZE};
use ssz_derive::{Decode, Encode};
use types::test_utils::{SeedableRng, TestRandom, XorShiftRng};
use types::{Hash256, PublicKeyBytes};

/// Simple datatype for fuzzing
#[derive(Debug, Clone, Default, Encode, Decode)]
#[ssz(struct_behaviour = "transparent")]
struct FuzzData(u64);

impl QbftData for FuzzData {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let mut hasher = Sha256::new();
        hasher.update(self.0.to_le_bytes());
        let hash: [u8; 32] = hasher.finalize().into();
        Hash256::from(hash)
    }

    fn validate(&self) -> bool {
        true
    }
}

/// All of the strategies to generate the test data
fn arb_operator_id() -> impl Strategy<Value = OperatorId> {
    (1..100u64).prop_map(OperatorId)
}

fn arb_committee(min: usize, max: usize) -> impl Strategy<Value = IndexSet<OperatorId>> {
    prop::collection::vec(arb_operator_id(), min..=max).prop_map(|v| IndexSet::from_iter(v))
}

fn arb_fuzz_data() -> impl Strategy<Value = FuzzData> {
    prop::num::u64::ANY.prop_map(FuzzData)
}

fn arb_qbft_message_type() -> impl Strategy<Value = QbftMessageType> {
    prop_oneof![
        Just(QbftMessageType::Proposal),
        Just(QbftMessageType::Prepare),
        Just(QbftMessageType::Commit),
        Just(QbftMessageType::RoundChange),
    ]
}

fn arb_domain_type() -> impl Strategy<Value = DomainType> {
    prop::array::uniform4(prop::num::u8::ANY).prop_map(DomainType::from)
}

fn arb_role() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::Committee),
        Just(Role::Aggregator),
        Just(Role::Proposer),
        Just(Role::SyncCommittee),
        Just(Role::ValidatorRegistration),
        Just(Role::VoluntaryExit),
    ]
}

fn arb_public_key() -> impl Strategy<Value = PublicKeyBytes> {
    // generate random seed to pass to rng
    prop::array::uniform::<_, 16>(prop::num::u8::ANY).prop_map(|seed| {
        let rng = &mut XorShiftRng::from_seed(seed);
        PublicKeyBytes::random_for_test(rng)
    })
}

fn arb_duty_executor(operators: Vec<OperatorId>) -> impl Strategy<Value = DutyExecutor> {
    prop_oneof![
        //(operators).prop_map(CommitteeId::from),
        (arb_public_key()).prop_map(DutyExecutor::Validator)
    ]
}

fn arb_message_id(operators: Vec<OperatorId>) -> impl Strategy<Value = MessageId> {
    (arb_domain_type(), arb_role(), arb_duty_executor(operators)).prop_flat_map(
        |(domain, role, executor)| Just(MessageId::new(&domain, role, &executor)).boxed(),
    )
}

fn arb_config() -> impl Strategy<Value = Config<DefaultLeaderFunction>> {
    (
        arb_operator_id(),
        prop::num::usize::ANY.prop_map(InstanceHeight::from),
        arb_committee(4, 13),
    )
        .prop_flat_map(|(operator_id, instance_height, committee)| {
            Just(
                ConfigBuilder::new(operator_id, instance_height, committee)
                    .build()
                    .unwrap(),
            )
            .boxed()
        })
}

// Generate a random QbftMessage
fn arb_qbft_message(operators: Vec<OperatorId>) -> impl Strategy<Value = QbftMessage> {
    (
        arb_qbft_message_type(),
        prop::num::u64::ANY, // height
        prop::num::u64::ANY.prop_filter("Round cannot be zero", |r| *r > 0), // round
        arb_message_id(operators),
        prop::array::uniform32(prop::num::u8::ANY).prop_map(Hash256::from), // root
        prop::num::u64::ANY,                                                // data_round
    )
        .prop_map(|(msg_type, height, round, identifier, root, data_round)| {
            QbftMessage {
                qbft_message_type: msg_type,
                height,
                round,
                identifier: (&identifier).into(),
                root,
                data_round,
                round_change_justification: Vec::new(), // Empty for simplicity
                prepare_justification: Vec::new(),      // Empty for simplicity
            }
        })
}

// Generate a signed SSV message for the QBFT instance
fn arb_signed_ssv_message(
    committee_members: &IndexSet<OperatorId>,
) -> impl Strategy<Value = SignedSSVMessage> + '_ {
    // Choose a random committee member
    prop::sample::select(committee_members.iter().cloned().collect::<Vec<_>>()).prop_flat_map(
        move |operator_id| {
            (
                arb_qbft_message(),
                Just(operator_id),
                prop::collection::vec(prop::num::u8::ANY, RSA_SIGNATURE_SIZE..=RSA_SIGNATURE_SIZE),
            )
                .prop_map(|(qbft_message, operator_id, signature_bytes)| {
                    // Create an SSV message
                    let ssv_message = SSVMessage::new(
                        MsgType::SSVConsensusMsgType,
                        MessageId::from([0u8; 56]),
                        qbft_message.as_ssz_bytes(),
                    )
                    .unwrap();

                    // Create a signed SSV message
                    SignedSSVMessage::new(
                        vec![signature_bytes],
                        vec![operator_id],
                        ssv_message,
                        Vec::new(), // Empty full_data for simplicity
                    )
                    .unwrap()
                })
        },
    )
}

// Generate a wrapped QBFT message
fn arb_wrapped_qbft_message(
    committee_members: &IndexSet<OperatorId>,
) -> impl Strategy<Value = WrappedQbftMessage> + '_ {
    arb_signed_ssv_message(committee_members).prop_flat_map(move |signed_message| {
        // Try to decode the QbftMessage from the SSVMessage
        match QbftMessage::from_ssz_bytes(signed_message.ssv_message().data()) {
            Ok(qbft_message) => Just(WrappedQbftMessage {
                signed_message,
                qbft_message,
            })
            .boxed(),
            Err(_) => proptest::strategy::empty().boxed(), // Reject invalid messages
        }
    })
}
