use crate::{Config, ConfigBuilder, DefaultLeaderFunction, InstanceHeight};
use crate::{UnsignedWrappedQbftMessage, WrappedQbftMessage};
use proptest::prelude::*;
use sha2::{Digest, Sha256};
use ssv_types::consensus::{QbftData, QbftMessage, QbftMessageType};
use ssv_types::domain_type::DomainType;
use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage, RSA_SIGNATURE_SIZE};
use ssv_types::msgid::{DutyExecutor, MessageId, Role};
use ssv_types::{CommitteeId, IndexSet, OperatorId};
use ssz::{Decode, Encode};
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
fn arb_operator_id(committee: Vec<OperatorId>) -> impl Strategy<Value = OperatorId> {
    prop::sample::select(committee)
}

fn arb_committee() -> impl Strategy<Value = Vec<OperatorId>> {
    prop_oneof![Just(4usize), Just(7usize), Just(10usize), Just(13usize)].prop_flat_map(|size| {
        prop::collection::vec(1..200u64, size..=size).prop_map(|nums| {
            nums.into_iter()
                .map(|num| OperatorId::from(num))
                .collect::<Vec<_>>()
        })
    })
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
    prop::array::uniform::<_, 16>(prop::num::u8::ANY).prop_map(|seed| {
        let rng = &mut XorShiftRng::from_seed(seed);
        PublicKeyBytes::random_for_test(rng)
    })
}

fn arb_duty_executor(operators: Vec<OperatorId>) -> impl Strategy<Value = DutyExecutor> {
    prop_oneof![
        arb_operator_id(operators.clone())
            .prop_map(|op_id| DutyExecutor::Committee(CommitteeId::from(vec![op_id]))),
        arb_public_key().prop_map(DutyExecutor::Validator)
    ]
}

fn arb_message_id(operators: Vec<OperatorId>) -> impl Strategy<Value = MessageId> {
    (arb_domain_type(), arb_role(), arb_duty_executor(operators)).prop_flat_map(
        |(domain, role, executor)| Just(MessageId::new(&domain, role, &executor)).boxed(),
    )
}

fn arb_config(
    committee: IndexSet<OperatorId>,
) -> impl Strategy<Value = Config<DefaultLeaderFunction>> {
    (
        arb_operator_id(committee.iter().cloned().collect::<Vec<_>>()),
        prop::num::usize::ANY.prop_map(InstanceHeight::from),
    )
        .prop_flat_map(move |(operator_id, instance_height)| {
            Just(
                ConfigBuilder::new(operator_id, instance_height, committee.clone())
                    .build()
                    .unwrap(),
            )
            .boxed()
        })
}

// Generate a random QbftMessage
fn arb_qbft_message(
    operators: Vec<OperatorId>,
    message_id: Option<MessageId>,
) -> impl Strategy<Value = QbftMessage> {
    let message_id_strategy = if let Some(msg_id) = message_id {
        Strategy::boxed(Just(msg_id))
    } else {
        Strategy::boxed(arb_message_id(operators.clone()))
    };

    (
        arb_qbft_message_type(),
        prop::num::u64::ANY, // height
        prop::num::u64::ANY.prop_filter("Round cannot be zero", |r| *r > 0), // round
        message_id_strategy,
        prop::array::uniform32(prop::num::u8::ANY).prop_map(Hash256::from), // root
        prop::num::u64::ANY,                                                // data_round
    )
        .prop_map(
            |(msg_type, height, round, identifier, root, data_round)| QbftMessage {
                qbft_message_type: msg_type,
                height,
                round,
                identifier: (&identifier).into(),
                root,
                data_round,
                round_change_justification: Vec::new(),
                prepare_justification: Vec::new(),
            },
        )
}

// Generate a signed SSV message for the QBFT instance
fn arb_signed_ssv_message(
    operators: Vec<OperatorId>,
    fixed_message_id: Option<MessageId>,
) -> impl Strategy<Value = SignedSSVMessage> {
    // Choose a random committee member and generate a message
    (
        prop::sample::select(operators.clone()),
        arb_qbft_message(operators.clone(), fixed_message_id.clone()),
        prop::collection::vec(prop::num::u8::ANY, RSA_SIGNATURE_SIZE..=RSA_SIGNATURE_SIZE),
    )
        .prop_map(move |(operator_id, qbft_message, signature_bytes)| {
            // Use the fixed message ID if provided, otherwise use a default
            let message_id = if let Some(id) = &fixed_message_id {
                id.clone()
            } else {
                MessageId::from([0u8; 56])
            };

            // Create an SSV message
            let ssv_message = SSVMessage::new(
                MsgType::SSVConsensusMsgType,
                message_id,
                qbft_message.as_ssz_bytes(),
            )
            .unwrap();

            // Create a signed SSV message
            SignedSSVMessage::new(
                vec![signature_bytes],
                vec![operator_id],
                ssv_message,
                Vec::new(),
            )
            .unwrap()
        })
}

// Generate a wrapped QBFT message with a specific message ID
fn arb_wrapped_qbft_message(
    committee: IndexSet<OperatorId>,
    message_id: Option<MessageId>,
) -> impl Strategy<Value = WrappedQbftMessage> {
    let operators: Vec<OperatorId> = committee.into_iter().collect();

    // Use proper strategy construction to avoid lifetime issues
    let strategy = arb_signed_ssv_message(operators, message_id);

    strategy.prop_map(|signed_message| {
        // Try to decode the QbftMessage from the SSVMessage
        match QbftMessage::from_ssz_bytes(signed_message.ssv_message().data()) {
            Ok(qbft_message) => WrappedQbftMessage {
                signed_message,
                qbft_message,
            },
            Err(e) => panic!(
                "Failed to decode QbftMessage: {:?}. This is a bug in the test suite.",
                e
            ),
        }
    })
}

// Generate proposal message for a specific round
fn arb_proposal_message(
    committee: IndexSet<OperatorId>,
    message_id: MessageId,
    data_hash: Hash256,
    round: u64,
) -> impl Strategy<Value = WrappedQbftMessage> {
    let operators: Vec<OperatorId> = committee.into_iter().collect();
    let leader_idx = (round - 1) as usize % operators.len();
    let leader = operators[leader_idx];

    (prop::collection::vec(prop::num::u8::ANY, RSA_SIGNATURE_SIZE..=RSA_SIGNATURE_SIZE)).prop_map(
        move |signature_bytes| {
            let qbft_message = QbftMessage {
                qbft_message_type: QbftMessageType::Proposal,
                height: 1, // Use 1 for testing purposes
                round,
                identifier: (&message_id).into(),
                root: data_hash,
                data_round: 0,
                round_change_justification: Vec::new(),
                prepare_justification: Vec::new(),
            };

            let ssv_message = SSVMessage::new(
                MsgType::SSVConsensusMsgType,
                message_id.clone(),
                qbft_message.as_ssz_bytes(),
            )
            .unwrap();

            let signed_message =
                SignedSSVMessage::new(vec![signature_bytes], vec![leader], ssv_message, Vec::new())
                    .unwrap();

            WrappedQbftMessage {
                signed_message,
                qbft_message,
            }
        },
    )
}

// Generate prepare message from a specific operator
fn arb_prepare_message(
    operator_id: OperatorId,
    message_id: MessageId,
    data_hash: Hash256,
    round: u64,
) -> impl Strategy<Value = WrappedQbftMessage> {
    (prop::collection::vec(prop::num::u8::ANY, RSA_SIGNATURE_SIZE..=RSA_SIGNATURE_SIZE)).prop_map(
        move |signature_bytes| {
            let qbft_message = QbftMessage {
                qbft_message_type: QbftMessageType::Prepare,
                height: 1, // Use 1 for testing purposes
                round,
                identifier: (&message_id).into(),
                root: data_hash,
                data_round: 0,
                round_change_justification: Vec::new(),
                prepare_justification: Vec::new(),
            };

            let ssv_message = SSVMessage::new(
                MsgType::SSVConsensusMsgType,
                message_id.clone(),
                qbft_message.as_ssz_bytes(),
            )
            .unwrap();

            let signed_message = SignedSSVMessage::new(
                vec![signature_bytes],
                vec![operator_id],
                ssv_message,
                Vec::new(),
            )
            .unwrap();

            WrappedQbftMessage {
                signed_message,
                qbft_message,
            }
        },
    )
}

// Generate commit message from a specific operator
fn arb_commit_message(
    operator_id: OperatorId,
    message_id: MessageId,
    data_hash: Hash256,
    round: u64,
) -> impl Strategy<Value = WrappedQbftMessage> {
    (prop::collection::vec(prop::num::u8::ANY, RSA_SIGNATURE_SIZE..=RSA_SIGNATURE_SIZE)).prop_map(
        move |signature_bytes| {
            let qbft_message = QbftMessage {
                qbft_message_type: QbftMessageType::Commit,
                height: 1, // Use 1 for testing purposes
                round,
                identifier: (&message_id).into(),
                root: data_hash,
                data_round: 0,
                round_change_justification: Vec::new(),
                prepare_justification: Vec::new(),
            };

            let ssv_message = SSVMessage::new(
                MsgType::SSVConsensusMsgType,
                message_id.clone(),
                qbft_message.as_ssz_bytes(),
            )
            .unwrap();

            let signed_message = SignedSSVMessage::new(
                vec![signature_bytes],
                vec![operator_id],
                ssv_message,
                Vec::new(),
            )
            .unwrap();

            WrappedQbftMessage {
                signed_message,
                qbft_message,
            }
        },
    )
}

// Generate round change message from a specific operator
fn arb_round_change_message(
    operator_id: OperatorId,
    message_id: MessageId,
    data_hash: Hash256,
    round: u64,
) -> impl Strategy<Value = WrappedQbftMessage> {
    (prop::collection::vec(prop::num::u8::ANY, RSA_SIGNATURE_SIZE..=RSA_SIGNATURE_SIZE)).prop_map(
        move |signature_bytes| {
            let qbft_message = QbftMessage {
                qbft_message_type: QbftMessageType::RoundChange,
                height: 1, // Use 1 for testing purposes
                round,
                identifier: (&message_id).into(),
                root: data_hash,
                data_round: 0,
                round_change_justification: Vec::new(),
                prepare_justification: Vec::new(),
            };

            let ssv_message = SSVMessage::new(
                MsgType::SSVConsensusMsgType,
                message_id.clone(),
                qbft_message.as_ssz_bytes(),
            )
            .unwrap();

            let signed_message = SignedSSVMessage::new(
                vec![signature_bytes],
                vec![operator_id],
                ssv_message,
                Vec::new(),
            )
            .unwrap();

            WrappedQbftMessage {
                signed_message,
                qbft_message,
            }
        },
    )
}

// Helper to create messages for each operator in a committee
fn generate_committee_messages<F>(
    committee: &IndexSet<OperatorId>,
    message_generator: F,
    message_id: &MessageId,
    data_hash: Hash256,
    round: u64,
) -> Vec<WrappedQbftMessage>
where
    F: Fn(OperatorId, MessageId, Hash256, u64) -> WrappedQbftMessage,
{
    committee
        .iter()
        .map(|&operator_id| message_generator(operator_id, message_id.clone(), data_hash, round))
        .collect()
}

// Simulated message handler that just counts messages
struct MessageCounter {
    count: usize,
    messages: Vec<UnsignedWrappedQbftMessage>,
}

impl MessageCounter {
    fn new() -> Self {
        Self {
            count: 0,
            messages: Vec::new(),
        }
    }
    fn handle_message(&mut self, msg: UnsignedWrappedQbftMessage) {
        self.count += 1;
        self.messages.push(msg);
    }
}

// Create a Strategy that generates (committee, config, data, msg_id) as a tuple
fn arb_qbft_instance_data(
) -> impl Strategy<Value = (Config<DefaultLeaderFunction>, FuzzData, MessageId)> {
    arb_committee().prop_flat_map(|committee_vec| {
        // Convert Vec<OperatorId> to IndexSet<OperatorId> for config generation
        let committee: IndexSet<OperatorId> = committee_vec.clone().into_iter().collect();

        // Return a tuple strategy with all the values we need
        (
            arb_config(committee),
            arb_fuzz_data(),
            arb_message_id(committee_vec),
        )
    })
}

proptest! {
    #[test]
    fn test_qbft_instance_creation(
        (config, data, msg_id) in arb_qbft_instance_data()
    ) {
        let mut counter = MessageCounter::new();
        let qbft = crate::Qbft::new(
            config.clone(),
            data.clone(),
            msg_id.clone(),
            |msg| counter.handle_message(msg),
        );

        // Verify that the instance was created with the expected configuration
        prop_assert_eq!(qbft.config().operator_id(), config.operator_id());
        prop_assert_eq!(qbft.config().instance_height(), config.instance_height());
        prop_assert_eq!(qbft.config().committee_members(), config.committee_members());
        prop_assert_eq!(qbft.start_data_hash(), &data.hash());
    }
}
