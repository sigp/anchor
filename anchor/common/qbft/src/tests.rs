//! A collection of unit tests for the QBFT Protocol.
//!
//! These test individual components and also provide full end-to-end tests of the entire protocol.

use std::{
    cell::RefCell,
    collections::{HashSet, VecDeque},
    rc::Rc,
};

use qbft_types::DefaultLeaderFunction;
use sha2::{Digest, Sha256};
use ssv_types::{
    OperatorId,
    consensus::{NoDataValidation, QbftMessage, QbftMessageType},
    message::{MsgType, RSA_SIGNATURE_SIZE, SSVMessage, SignedSSVMessage},
};
use ssz_derive::{Decode, Encode};
use tracing::debug_span;
use tracing_subscriber::filter::EnvFilter;
use types::Hash256;

use super::*;

// HELPER FUNCTIONS FOR TESTS

/// Enable debug logging for tests
const ENABLE_TEST_LOGGING: bool = true;

/// Initialize test logging (call once per test that needs logging)
fn init_test_logging() {
    if ENABLE_TEST_LOGGING {
        let env_filter = EnvFilter::new("debug");
        let _ = tracing_subscriber::fmt()
            .compact()
            .with_env_filter(env_filter)
            .try_init();
    }
}

/// Create a basic 3-node QBFT instance for testing
fn create_test_qbft_instance(
    test_data_value: u64,
) -> Qbft<DefaultLeaderFunction, TestData, impl FnMut(UnsignedWrappedQbftMessage)> {
    let config = ConfigBuilder::<DefaultLeaderFunction>::new(
        1.into(),
        InstanceHeight::default(),
        (1..4).map(OperatorId::from).collect(), // 3 nodes
    )
    .with_operator_id(OperatorId::from(1))
    .build()
    .expect("config should be valid");

    let test_data = TestData(test_data_value);
    Qbft::new(
        config,
        test_data,
        Box::new(NoDataValidation),
        MessageId::from([0; 56]),
        |_| {},
    )
}

/// Test data structure that implements the Data trait
#[derive(Debug, Clone, Default, Encode, Decode)]
#[ssz(struct_behaviour = "transparent")]
struct TestData(u64);

impl QbftData for TestData {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let mut hasher = Sha256::new();
        hasher.update(self.0.to_le_bytes());
        let hash: [u8; 32] = hasher.finalize().into();
        Hash256::from(hash)
    }
}

fn convert_unsigned_to_signed(
    msg: UnsignedWrappedQbftMessage,
    operator_id: OperatorId,
) -> WrappedQbftMessage {
    // Create a signed message containing just this operator
    let signed_message = SignedSSVMessage::new(
        vec![vec![0; RSA_SIGNATURE_SIZE]],
        vec![OperatorId(*operator_id)],
        msg.unsigned_message.ssv_message,
        msg.unsigned_message.full_data,
    )
    .expect("Should create signed message");

    WrappedQbftMessage {
        signed_message,
        qbft_message: msg.qbft_message,
    }
}

/// A struct to help build and initialise a test of running instances
struct TestQBFTCommitteeBuilder {
    /// The configuration to use for all the instances.
    config: ConfigBuilder,
}

impl Default for TestQBFTCommitteeBuilder {
    fn default() -> Self {
        TestQBFTCommitteeBuilder {
            config: ConfigBuilder::new(
                1.into(),
                InstanceHeight::default(),
                (1..6).map(OperatorId::from).collect(),
            ),
        }
    }
}

#[allow(dead_code)]
impl TestQBFTCommitteeBuilder {
    /// Consumes self and runs a test scenario. This returns a [`TestQBFTCommittee`] which
    /// represents a running quorum.
    pub fn run<D>(self, data: D) -> TestQBFTCommittee<D, impl FnMut(UnsignedWrappedQbftMessage)>
    where
        D: Default + QbftData<Hash = Hash256>,
    {
        init_test_logging();
        construct_and_run_committee(self.config, data)
    }
}

/// A testing structure representing a committee of running instances
#[allow(clippy::type_complexity)]
struct TestQBFTCommittee<D: QbftData<Hash = Hash256>, S: FnMut(UnsignedWrappedQbftMessage)> {
    msg_queue: Rc<RefCell<VecDeque<(OperatorId, UnsignedWrappedQbftMessage)>>>,
    instances: HashMap<OperatorId, Qbft<DefaultLeaderFunction, D, S>>,
    // All of the instances that are currently active, allows us to stop/restart instances by
    // controlling the messages being sent and received
    active_instances: HashSet<OperatorId>,
}

/// Constructs and runs committee of QBFT Instances
///
/// This will create instances and spawn them in a task and return the sender/receiver channels for
/// all created instances.
fn construct_and_run_committee<D: QbftData<Hash = Hash256>>(
    mut config: ConfigBuilder,
    validated_data: D,
) -> TestQBFTCommittee<D, impl FnMut(UnsignedWrappedQbftMessage)> {
    // The ID of a committee is just an integer in [0,committee_size)

    let msg_queue = Rc::new(RefCell::new(VecDeque::new()));
    let mut instances = HashMap::with_capacity(config.committee_members().len());
    let mut active_instances = HashSet::new();

    for id in 1..config.committee_members().len() + 1 {
        let msg_queue = Rc::clone(&msg_queue);
        let id = OperatorId::from(id as u64);
        // Creates a new instance
        config = config.with_operator_id(id);
        let instance = Qbft::new(
            config.clone().build().expect("test config is valid"),
            validated_data.clone(),
            Box::new(NoDataValidation),
            MessageId::from([0; 56]),
            move |message| msg_queue.borrow_mut().push_back((id, message)),
        );
        instances.insert(id, instance);
        active_instances.insert(id);
    }

    TestQBFTCommittee {
        msg_queue,
        instances,
        active_instances,
    }
}

impl<D: QbftData<Hash = Hash256>, S: FnMut(UnsignedWrappedQbftMessage)> TestQBFTCommittee<D, S> {
    fn wait_until_end(mut self) -> i32 {
        loop {
            let msg = self.msg_queue.borrow_mut().pop_front();
            let Some((sender, msg)) = msg else {
                // we are done! check how many instances reached consensus
                let mut num_consensus = 0;
                for id in self.active_instances.iter() {
                    let instance = self.instances.get_mut(id).expect("Instance exists");
                    // Check if this instance just reached consensus
                    if matches!(instance.completed, Some(Completed::Success(_))) {
                        num_consensus += 1;
                    }
                }
                return num_consensus;
            };

            // Only receive messages for active instances
            for id in self.active_instances.iter() {
                let span = debug_span!("receive", self = ?id);

                // We do not make sure that id != sender since we want to loop back and receive our
                // own messages
                let instance = self.instances.get_mut(id).expect("Instance exists");

                let wrapped = convert_unsigned_to_signed(msg.clone(), sender);
                span.in_scope(|| instance.receive(wrapped));
            }
        }
    }

    // Pause an qbft instance from running. This will simulate the node going down
    pub fn pause_instance(&mut self, id: &OperatorId) {
        self.active_instances.remove(id);
    }

    /// Restart a paused qbft instance. This will simulate it coming back online
    pub fn restart_instance(&mut self, id: &OperatorId) {
        self.active_instances.insert(*id);
    }
}

#[test]
// Construct and run a test committee
fn test_basic_committee() {
    let test_instance = TestQBFTCommitteeBuilder::default().run(TestData(21));

    // Wait until consensus is reached or all the instances have ended
    let num_consensus = test_instance.wait_until_end();
    assert!(num_consensus == 5);
}

#[test]
// Test consensus recovery with F faulty operators
fn test_consensus_with_f_faulty_operators() {
    let mut test_instance = TestQBFTCommitteeBuilder::default().run(TestData(21));

    test_instance.pause_instance(&OperatorId::from(2));

    // Wait until consensus is reached or all the instances have ended
    let num_consensus = test_instance.wait_until_end();
    assert!(num_consensus == 4);
}

#[test]
fn test_node_recovery() {
    let mut test_instance = TestQBFTCommitteeBuilder::default().run(TestData(42));

    // Pause a node
    test_instance.pause_instance(&OperatorId::from(2));

    // Then restart it
    test_instance.restart_instance(&OperatorId::from(2));

    let num_consensus = test_instance.wait_until_end();
    assert_eq!(num_consensus, 5); // Should reach full consensus after recovery
}

#[test]
/// Test that FAILS if round change validation doesn't require prepare justifications for
/// data_round=1
///
/// This test creates a proposal with round change messages claiming preparation in round 1
/// (data_round=1) but provides NO prepare justifications.
/// The test FAILS if the validation doesn't reject the proposal as it should.
fn test_round_change_validation_skips_round_one_prepared_values() {
    if ENABLE_TEST_LOGGING {
        let env_filter = EnvFilter::new("debug");
        let _ = tracing_subscriber::fmt()
            .compact()
            .with_env_filter(env_filter)
            .try_init();
    }

    use ssv_types::{
        consensus::QbftMessage,
        message::{MsgType, RSA_SIGNATURE_SIZE, SSVMessage, SignedSSVMessage},
    };

    // Create QBFT instance
    let config = ConfigBuilder::<DefaultLeaderFunction>::new(
        1.into(),
        InstanceHeight::default(),
        (1..4).map(OperatorId::from).collect(), // 3 nodes, quorum = 3
    )
    .with_operator_id(OperatorId::from(1))
    .build()
    .expect("config should be valid");

    let test_data = TestData(123);
    let qbft_instance = Qbft::new(
        config,
        test_data.clone(),
        Box::new(NoDataValidation),
        MessageId::from([0; 56]),
        |_| {},
    );

    // Create a MALICIOUS round change message:
    // - Claims to have prepared a value in round 1 (data_round = 1)
    // - But provides NO prepare justifications (empty prepare_justification)
    // This should be REJECTED but the bug allows it through
    let malicious_round_change = QbftMessage {
        qbft_message_type: QbftMessageType::RoundChange,
        height: 0,
        round: 2,
        identifier: [0; 56].to_vec().into(),
        root: test_data.hash(),
        data_round: 1, // Claims preparation in round 1 - this is the bug trigger!
        round_change_justification: vec![],
        prepare_justification: vec![], // INVALID: No justifications for claimed preparation!
    };

    // Create signed round change messages (need quorum of 3 for 3-node committee)
    let mut signed_round_changes = vec![];
    for operator_id in [1, 2, 3] {
        // Create the SSVMessage properly
        let ssv_message = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            MessageId::from([0; 56]),
            malicious_round_change.as_ssz_bytes(),
        )
        .expect("should create SSVMessage");

        let signed_rc = SignedSSVMessage::new(
            vec![vec![0; RSA_SIGNATURE_SIZE]],
            vec![OperatorId::from(operator_id)],
            ssv_message,
            vec![], // no full_data for round change
        )
        .expect("should create signed message");
        signed_round_changes.push(signed_rc);
    }

    // Create proposal that includes these invalid round changes
    let proposal = QbftMessage {
        qbft_message_type: QbftMessageType::Proposal,
        height: 0,
        round: 2,
        identifier: [0; 56].to_vec().into(),
        root: test_data.hash(),
        data_round: 1, // Proposing the "prepared" value from round 1
        round_change_justification: signed_round_changes,
        prepare_justification: vec![], // Proposals don't need prepare justifications
    };

    // Create the SSVMessage for the proposal
    let proposal_ssv_message = SSVMessage::new(
        MsgType::SSVConsensusMsgType,
        MessageId::from([0; 56]),
        proposal.as_ssz_bytes(),
    )
    .expect("should create proposal SSVMessage");

    let signed_proposal = SignedSSVMessage::new(
        vec![vec![0; RSA_SIGNATURE_SIZE]],
        vec![OperatorId::from(2)], // From operator 2 (leader for round 2)
        proposal_ssv_message,
        test_data.as_ssz_bytes(), // full_data for proposal
    )
    .expect("should create signed proposal");

    let wrapped_proposal = WrappedQbftMessage {
        signed_message: signed_proposal,
        qbft_message: proposal,
    };

    // Call the actual buggy validation function
    let validation_result = qbft_instance.validate_proposal_justifications(&wrapped_proposal);

    // The validation should REJECT this proposal because:
    // - Round change messages claim data_round=1 (prepared in round 1)
    // - But they provide NO prepare justifications to prove this claim

    println!(
        "Validation result for malicious proposal: {}",
        validation_result
    );
    println!("This proposal should be REJECTED because round change messages");
    println!("claim preparation in round 1 but provide no prepare justifications.");

    // This assertion will FAIL if a buggy code returns true (accepts invalid proposal)
    assert!(
        !validation_result,
        "BUG: validate_justifications() accepted an invalid proposal! \
         Round change messages claim data_round=1 (prepared in round 1) but provide no \
         prepare justifications. This should be rejected but the validation logic \
         incorrectly skips prepare justification checking for round 1 preparations."
    );
}

/// Test that RoundChange messages with prepared_round >= round are properly rejected
///
/// The QBFT specification requires that prepared_round (data_round) must be strictly less than
/// the current round to prevent circular justifications. This test verifies that RoundChange
/// messages with data_round >= round are correctly rejected by the QBFT instance.
#[test]
fn test_round_change_rejects_prepared_round_equal_to_current_round() {
    init_test_logging();

    let mut qbft_instance = create_test_qbft_instance(456);
    let test_data = TestData(456);

    // Create a RoundChange message that violates the spec:
    // prepared_round (data_round) equals the current round
    let invalid_round_change = QbftMessage {
        qbft_message_type: QbftMessageType::RoundChange,
        height: 0,
        round: 2, // Current round is 2
        identifier: [0; 56].to_vec().into(),
        root: test_data.hash(),
        data_round: 2, // INVALID: prepared_round == round (should be < round)
        round_change_justification: vec![],
        prepare_justification: vec![],
    };

    // Create the SSVMessage wrapper
    let ssv_message = SSVMessage::new(
        MsgType::SSVConsensusMsgType,
        MessageId::from([0; 56]),
        invalid_round_change.as_ssz_bytes(),
    )
    .expect("should create SSVMessage");

    let signed_round_change = SignedSSVMessage::new(
        vec![vec![0; RSA_SIGNATURE_SIZE]],
        vec![OperatorId::from(2)], // From operator 2
        ssv_message,
        vec![], // No full_data for round change
    )
    .expect("should create signed message");

    let wrapped_msg = WrappedQbftMessage {
        signed_message: signed_round_change,
        qbft_message: invalid_round_change,
    };

    // Process the message - it should be rejected
    qbft_instance.receive(wrapped_msg);

    // Verify the instance did not process the invalid message
    // The instance should still be in its initial state (not advanced to round 2)
    assert_eq!(
        qbft_instance.current_round,
        1.into(),
        "BUG: QBFT instance processed invalid RoundChange message with data_round >= round! \
         The message should have been rejected according to QBFT spec requirement that \
         prepared_round < round, but the instance advanced to round 2."
    );

    // Verify the instance is still waiting (not completed due to invalid message)
    assert!(
        qbft_instance.completed.is_none(),
        "BUG: QBFT instance completed consensus after receiving invalid RoundChange! \
         The message with data_round >= round should have been rejected."
    );
}

#[test]
/// Test that RoundChange messages with prepared_round > round are also properly rejected
///
/// This complements the previous test by checking that data_round > round is also rejected,
/// ensuring the validation covers the full >= condition.
fn test_round_change_rejects_prepared_round_greater_than_current_round() {
    init_test_logging();

    let mut qbft_instance = create_test_qbft_instance(789);
    let test_data = TestData(789);

    // Create RoundChange message with data_round > round (also invalid)
    let invalid_round_change = QbftMessage {
        qbft_message_type: QbftMessageType::RoundChange,
        height: 0,
        round: 2, // Current round is 2
        identifier: [0; 56].to_vec().into(),
        root: test_data.hash(),
        data_round: 3, // INVALID: prepared_round > round (should be < round)
        round_change_justification: vec![],
        prepare_justification: vec![],
    };

    let ssv_message = SSVMessage::new(
        MsgType::SSVConsensusMsgType,
        MessageId::from([0; 56]),
        invalid_round_change.as_ssz_bytes(),
    )
    .expect("should create SSVMessage");

    let signed_round_change = SignedSSVMessage::new(
        vec![vec![0; RSA_SIGNATURE_SIZE]],
        vec![OperatorId::from(3)],
        ssv_message,
        vec![],
    )
    .expect("should create signed message");

    let wrapped_msg = WrappedQbftMessage {
        signed_message: signed_round_change,
        qbft_message: invalid_round_change,
    };

    // Process the invalid message
    qbft_instance.receive(wrapped_msg);

    // Verify rejection - should remain in initial round
    assert_eq!(
        qbft_instance.current_round,
        1.into(),
        "BUG: QBFT instance processed invalid RoundChange message with data_round > round! \
         The message should have been rejected."
    );

    assert!(
        qbft_instance.completed.is_none(),
        "BUG: QBFT instance completed after receiving invalid message."
    );
}

/// Test that proposal validation correctly handles mixed RoundChange messages with different
/// prepared values This test verifies that only the HIGHEST prepared RoundChange must match the
/// proposal root, not ALL RoundChanges as the old implementation incorrectly required.
///
/// Scenario:
/// - Node 1: prepared value A in round 1 (data_round=1, root=A)
/// - Node 2: prepared value B in round 2 (data_round=2, root=B) <- highest prepared
/// - Node 3: no prepared value (data_round=0)
/// - Proposal: proposes value B (matching highest prepared)
///
/// This should be VALID according to QBFT spec - proposal must use highest prepared value
#[test]
fn test_mixed_round_change_values_highest_prepared_wins() {
    use ssv_types::{
        consensus::{QbftMessage, QbftMessageType},
        message::{MsgType, RSA_SIGNATURE_SIZE, SSVMessage, SignedSSVMessage},
    };

    // Create QBFT instance with 3-node committee
    let config = ConfigBuilder::<DefaultLeaderFunction>::new(
        1.into(),
        InstanceHeight::default(),
        (1..4).map(OperatorId::from).collect(), // 3 nodes
    )
    .with_operator_id(OperatorId::from(1))
    .build()
    .expect("config should be valid");

    let test_data = TestData(42);
    let qbft_instance = Qbft::new(
        config,
        test_data.clone(),
        Box::new(NoDataValidation),
        MessageId::from([0; 56]),
        |_| {},
    );

    // Create two different test data values
    let test_data_a = TestData(100); // value_A
    let test_data_b = TestData(200); // value_B

    // Create prepare justifications for value A from round 1
    let prepare_justifications_a = create_prepare_justifications(1, test_data_a.hash(), &[1, 2, 3]);

    // Create prepare justifications for value B from round 2
    let prepare_justifications_b = create_prepare_justifications(2, test_data_b.hash(), &[1, 2, 3]);

    // Create RoundChange from Node 1: prepared value A in round 1
    let rc1_msg = QbftMessage {
        qbft_message_type: QbftMessageType::RoundChange,
        height: 0,
        round: 3, // Current round
        identifier: [0; 56].to_vec().into(),
        root: test_data_a.hash(), // Prepared value A
        data_round: 1,            // Prepared in round 1
        round_change_justification: prepare_justifications_a,
        prepare_justification: vec![],
    };

    let rc1_signed = create_signed_round_change(&rc1_msg, 1);

    // Create RoundChange from Node 2: prepared value B in round 2 (HIGHEST)
    let rc2_msg = QbftMessage {
        qbft_message_type: QbftMessageType::RoundChange,
        height: 0,
        round: 3, // Current round
        identifier: [0; 56].to_vec().into(),
        root: test_data_b.hash(), // Prepared value B
        data_round: 2,            // Prepared in round 2 <- HIGHEST
        round_change_justification: prepare_justifications_b.clone(),
        prepare_justification: vec![],
    };

    let rc2_signed = create_signed_round_change(&rc2_msg, 2);

    // Create RoundChange from Node 3: no prepared value
    let rc3_msg = QbftMessage {
        qbft_message_type: QbftMessageType::RoundChange,
        height: 0,
        round: 3, // Current round
        identifier: [0; 56].to_vec().into(),
        root: Hash256::default(), // No prepared value
        data_round: 0,            // No prepared value
        round_change_justification: vec![],
        prepare_justification: vec![],
    };

    let rc3_signed = create_signed_round_change(&rc3_msg, 3);

    // Create proposal that uses value B (from highest prepared RoundChange)
    let proposal_msg = QbftMessage {
        qbft_message_type: QbftMessageType::Proposal,
        height: 0,
        round: 3,
        identifier: [0; 56].to_vec().into(),
        root: test_data_b.hash(), // Proposing value B (matches highest prepared)
        data_round: 2,            // From round 2 (highest prepared)
        round_change_justification: vec![rc1_signed, rc2_signed, rc3_signed],
        prepare_justification: prepare_justifications_b, /* Prepare justifications for value B
                                                          * from round 2 */
    };

    let proposal_ssv = SSVMessage::new(
        MsgType::SSVConsensusMsgType,
        MessageId::from([0; 56]),
        proposal_msg.as_ssz_bytes(),
    )
    .expect("should create SSVMessage");

    let signed_proposal = SignedSSVMessage::new(
        vec![vec![0; RSA_SIGNATURE_SIZE]],
        vec![OperatorId::from(1)], // From leader
        proposal_ssv,
        test_data_b.as_ssz_bytes(), // Include full data for value B
    )
    .expect("should create signed proposal");

    let wrapped_proposal = WrappedQbftMessage {
        signed_message: signed_proposal,
        qbft_message: proposal_msg,
    };

    // This should be VALID - proposal correctly uses highest prepared value
    let validation_result = qbft_instance.validate_proposal_justifications(&wrapped_proposal);

    assert!(
        validation_result,
        "Proposal validation should succeed when proposal uses highest prepared value. \
        Node 1 prepared A in round 1, Node 2 prepared B in round 2 (highest), \
        proposal correctly uses B"
    );
}

// Helper function to create prepare justifications for a given round and value
fn create_prepare_justifications(
    round: u64,
    root: Hash256,
    operator_ids: &[u64],
) -> Vec<SignedSSVMessage> {
    operator_ids
        .iter()
        .map(|&op_id| {
            let prepare_msg = QbftMessage {
                qbft_message_type: QbftMessageType::Prepare,
                height: 0,
                round,
                identifier: [0; 56].to_vec().into(),
                root,
                data_round: 0,
                round_change_justification: vec![],
                prepare_justification: vec![],
            };

            let ssv_msg = SSVMessage::new(
                MsgType::SSVConsensusMsgType,
                MessageId::from([0; 56]),
                prepare_msg.as_ssz_bytes(),
            )
            .expect("should create SSVMessage");

            SignedSSVMessage::new(
                vec![vec![0; RSA_SIGNATURE_SIZE]],
                vec![OperatorId::from(op_id)],
                ssv_msg,
                vec![],
            )
            .expect("should create signed prepare")
        })
        .collect()
}

// Helper function to create a signed round change message
fn create_signed_round_change(rc_msg: &QbftMessage, operator_id: u64) -> SignedSSVMessage {
    let ssv_msg = SSVMessage::new(
        MsgType::SSVConsensusMsgType,
        MessageId::from([0; 56]),
        rc_msg.as_ssz_bytes(),
    )
    .expect("should create SSVMessage");

    SignedSSVMessage::new(
        vec![vec![0; RSA_SIGNATURE_SIZE]],
        vec![OperatorId::from(operator_id)],
        ssv_msg,
        vec![],
    )
    .expect("should create signed round change")
}
