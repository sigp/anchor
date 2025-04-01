use crate::{Config, ConfigBuilder, DefaultLeaderFunction, InstanceHeight, Qbft};
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
    msg_id: MessageId,
    data: FuzzData,
) -> impl Strategy<Value = QbftMessage> {
    (
        arb_qbft_message_type(),
        prop::num::u64::ANY, // height
        prop::num::u64::ANY.prop_filter("Round cannot be zero", |r| *r > 0), // round
        Strategy::boxed(Just(msg_id)),
        prop::num::u64::ANY, // data_round
    )
        .prop_map(
            move |(msg_type, height, round, identifier, data_round)| QbftMessage {
                qbft_message_type: msg_type,
                height,
                round,
                identifier: (&identifier).into(),
                root: data.clone().hash(),
                data_round,
                round_change_justification: Vec::new(),
                prepare_justification: Vec::new(),
            },
        )
}

// Generate a signed SSV message for the QBFT instance
fn arb_signed_ssv_message(
    operators: Vec<OperatorId>,
    msg_id: MessageId,
    data: FuzzData,
) -> impl Strategy<Value = SignedSSVMessage> {
    // Choose a random committee member and generate a message
    (
        prop::sample::select(operators.clone()),
        arb_qbft_message(operators.clone(), msg_id.clone(), data.clone()),
        prop::collection::vec(prop::num::u8::ANY, RSA_SIGNATURE_SIZE..=RSA_SIGNATURE_SIZE),
    )
        .prop_map(move |(operator_id, qbft_message, signature_bytes)| {
            // Create an SSV message
            let ssv_message = SSVMessage::new(
                MsgType::SSVConsensusMsgType,
                msg_id.clone(),
                qbft_message.as_ssz_bytes(),
            )
            .unwrap();

            // Create a signed SSV message
            SignedSSVMessage::new(
                vec![signature_bytes], // todo, need a real sig?
                vec![operator_id],
                ssv_message,
                data.as_ssz_bytes(),
            )
            .unwrap()
        })
}

// Generate a wrapped QBFT message with a specific message ID and committee
fn arb_wrapped_qbft_message(
    committee: IndexSet<OperatorId>,
    message_id: MessageId,
    data: FuzzData,
) -> impl Strategy<Value = WrappedQbftMessage> {
    arb_signed_ssv_message(committee.into_iter().collect(), message_id, data.clone()).prop_map(
        |signed_message| match QbftMessage::from_ssz_bytes(signed_message.ssv_message().data()) {
            Ok(qbft_message) => WrappedQbftMessage {
                signed_message,
                qbft_message,
            },
            Err(e) => panic!(
                "Failed to decode QbftMessage: {:?}. This is a bug in the test suite.",
                e
            ),
        },
    )
}

// Generate an arbitrary QBFT configuration
fn arb_qbft_config() -> impl Strategy<Value = (Config<DefaultLeaderFunction>, FuzzData, MessageId)>
{
    arb_committee().prop_flat_map(|committee_vec| {
        (
            arb_config(committee_vec.clone().into_iter().collect()),
            arb_fuzz_data(),
            arb_message_id(committee_vec),
        )
    })
}

/// Helper to convert a SignedSSVMessage to a WrappedQbftMessage
fn make_wrapped_message(signed_message: SignedSSVMessage) -> WrappedQbftMessage {
    let qbft_message = QbftMessage::from_ssz_bytes(signed_message.ssv_message().data())
        .expect("Should be valid QBFT message");

    WrappedQbftMessage {
        signed_message,
        qbft_message,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10))]

    #[test]
    fn test_qbft_instance_creation(
        (config, data, msg_id) in arb_qbft_config()
    ) {
        let mut counter = MessageCounter::new();
        let qbft = Qbft::new(
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

        // Verify that the initial message handling worked
        prop_assert!(counter.count > 0, "Instance should have sent at least one message");
    }

    #[test]
    fn test_qbft_receive_messages(
        (config, data, msg_id) in arb_qbft_config()
    ) {
        let mut counter = MessageCounter::new();
        let mut qbft = Qbft::new(
            config.clone(),
            data.clone(),
            msg_id.clone(),
            |msg| counter.handle_message(msg),
        );

        // Generate a valid message for this instance
        let signed_message = arb_signed_ssv_message(
            config.committee_members().iter().cloned().collect(),
            msg_id.clone(),
            data.clone()
        )
        .new_tree(&mut proptest::test_runner::TestRunner::default())
        .unwrap();

        let wrapped_msg = make_wrapped_message(signed_message);

        // Make the round field match the instance's current round to pass validation
        let mut valid_msg = wrapped_msg.clone();
        //valid_msg.qbft_message.round = 1; // Default round is 1
        //valid_msg.qbft_message.height = *config.instance_height() as u64;

        // Receive the message
        qbft.receive(valid_msg);

        // No explicit assertion here since we're just testing that receiving
        // a message doesn't panic or crash
    }

    #[test]
    fn test_qbft_round_advancement(
        (config, data, msg_id) in arb_qbft_config()
    ) {
        let mut counter = MessageCounter::new();
        let mut qbft = Qbft::new(
            config.clone(),
            data.clone(),
            msg_id.clone(),
            |msg| counter.handle_message(msg),
        );

        // End the current round
        qbft.end_round();

        // Verify counter received a round change message
        prop_assert!(counter.count > 1, "Should have sent at least one more message after round end");

        let found_round_change = counter.messages.iter().any(|msg|
            matches!(msg.qbft_message.qbft_message_type, QbftMessageType::RoundChange)
        );

        prop_assert!(found_round_change, "Should have sent a round change message");
    }

    #[test]
    fn test_qbft_multiple_rounds(
        (mut config, data, msg_id) in arb_qbft_config()
    ) {
        // Set a higher max rounds to allow multiple round changes
        config = ConfigBuilder::new(
            config.operator_id(),
            *config.instance_height(),
            config.committee_members().clone()
        )
        .with_max_rounds(5)
        .build()
        .unwrap();

        let mut counter = MessageCounter::new();
        let mut qbft = Qbft::new(
            config.clone(),
            data.clone(),
            msg_id.clone(),
            |msg| counter.handle_message(msg),
        );

        // Progress through multiple rounds
        for _ in 0..3 {
            qbft.end_round();
        }

        // After multiple round changes, we should either have timed out or still be in progress
        let completed = qbft.completed();

        // If we completed, it should be with a timeout
        if let Some(completed) = completed {
            prop_assert!(matches!(completed, crate::Completed::TimedOut),
                         "If completed after multiple rounds, should be due to timeout");
        }
    }

    #[test]
    fn test_qbft_with_multiple_committee_sizes(
        committee_size in prop_oneof![Just(4usize), Just(7usize), Just(10usize), Just(13usize)],
        data_value in prop::num::u64::ANY,
    ) {
        // Generate committee members
        let committee_members: IndexSet<_> = (1..=committee_size as u64)
            .map(OperatorId::from)
            .collect();

        // Create config with arbitrary operator as leader
        let operator_id = OperatorId::from(1u64);
        let instance_height = InstanceHeight::from(1usize);

        let config = ConfigBuilder::new(
            operator_id,
            instance_height,
            committee_members.clone()
        )
        .build()
        .unwrap();

        // Create data and message ID
        let data = FuzzData(data_value);
        let domain = DomainType([0, 0, 0, 0]);
        let msg_id = MessageId::new(
            &domain,
            Role::Committee,
            &DutyExecutor::Committee(CommitteeId::from(
                committee_members.iter().cloned().collect::<Vec<_>>()
            ))
        );

        // Create QBFT instance
        let mut counter = MessageCounter::new();
        let qbft = Qbft::new(
            config.clone(),
            data.clone(),
            msg_id.clone(),
            |msg| counter.handle_message(msg),
        );

        // Check that quorum size is properly calculated based on committee size
        let f = (committee_size - 1) / 3;
        prop_assert_eq!(config.quorum_size(), committee_size - f);

        // Verify instance created successfully
        prop_assert_eq!(qbft.config().committee_members().len(), committee_size);
    }
}
