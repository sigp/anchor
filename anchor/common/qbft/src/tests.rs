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
    consensus::{NoDataValidation, QbftMessageType},
    message::{RSA_SIGNATURE_SIZE, SignedSSVMessage},
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
                span.in_scope(|| {
                    if let Err(e) = instance.receive(wrapped) {
                        debug!("Qbft error: {:?}", e);
                    }
                });
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
        "Validation result for malicious proposal: {:?}",
        validation_result
    );
    println!("This proposal should be REJECTED because round change messages");
    println!("claim preparation in round 1 but provide no prepare justifications.");

    // This assertion will FAIL if a buggy code returns true (accepts invalid proposal)
    assert!(
        validation_result.is_err(),
        "BUG: validate_justifications() accepted an invalid proposal! \
         Round change messages claim data_round=1 (prepared in round 1) but provide no \
         prepare justifications. This should be rejected but the validation logic \
         incorrectly skips prepare justification checking for round 1 preparations."
    );
}

#[test]
// Test that verifies correct QBFT spec behavior when leader has highest prepared RoundChange
// but the full_data is missing.
//
// This test directly validates the RcJustificationOutcome::PreparedExistsButDataMissing behavior:
// 1. Create a QBFT instance that will be the leader for a round
// 2. Manually add round change messages with highest prepared data but missing full_data
// 3. Verify that when the leader processes these messages, it does NOT send a proposal
//
// The test FAILS if the leader incorrectly proposes when highest prepared data is missing.
fn test_leader_waits_when_highest_prepared_data_missing() {
    init_test_logging();

    use std::sync::{Arc, Mutex};

    use ssv_types::{
        consensus::QbftMessage,
        message::{MsgType, RSA_SIGNATURE_SIZE, SSVMessage, SignedSSVMessage},
    };

    // Track messages sent by the QBFT instance
    let sent_messages = Arc::new(Mutex::new(Vec::new()));
    let sent_messages_clone = sent_messages.clone();

    // Create QBFT instance that will be leader for round 2
    // Leader for round R: committee[(R-1 + height) % committee_size]
    // Round 2: (2-1+0) % 3 = 1 -> operator 2 (index 1 in [1,2,3])
    let config = ConfigBuilder::<DefaultLeaderFunction>::new(
        1.into(),
        InstanceHeight::default(),
        (1..4).map(OperatorId::from).collect(), // [1, 2, 3]
    )
    .with_operator_id(OperatorId::from(2)) // This node is leader for round 2
    .build()
    .expect("config should be valid");

    let initial_data = TestData(100);
    let prepared_data = TestData(200); // Different data that was "prepared" in round 1
    let prepared_hash = prepared_data.hash();

    let mut qbft_instance = Qbft::new(
        config,
        initial_data.clone(),
        Box::new(NoDataValidation),
        MessageId::from([0; 56]),
        move |message| {
            sent_messages_clone.lock().unwrap().push(message);
        },
    );

    // Set up the scenario: we're moving to round 2 where node 2 is leader
    qbft_instance.current_round = 2.into();
    qbft_instance.state = InstanceState::AwaitingProposal;

    // Manually create and inject round change messages for round 2
    // These messages claim that prepared_data was prepared in round 1
    // but DO NOT provide the full_data (simulating the bug scenario)

    // Create the round change messages
    for operator_id in [1, 2, 3] {
        let round_change = QbftMessage {
            qbft_message_type: QbftMessageType::RoundChange,
            height: 0,
            round: 2, // Moving to round 2
            identifier: [0; 56].to_vec().into(),
            root: prepared_hash,                // Claims this hash was prepared
            data_round: 1,                      // Claims preparation happened in round 1
            round_change_justification: vec![], // No RC justifications needed for this test
            prepare_justification: vec![],      /* Should have prepare messages but we'll skip
                                                 * validation */
        };

        let ssv_message = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            MessageId::from([0; 56]),
            round_change.as_ssz_bytes(),
        )
        .expect("should create SSVMessage");

        let signed_rc = SignedSSVMessage::new(
            vec![vec![0; RSA_SIGNATURE_SIZE]],
            vec![OperatorId::from(operator_id)],
            ssv_message,
            vec![], // CRITICAL: No full_data - this simulates the missing data scenario!
        )
        .expect("should create signed message");

        let wrapped_rc = WrappedQbftMessage {
            signed_message: signed_rc,
            qbft_message: round_change,
        };

        // Directly add to round change container (bypassing full validation for test setup)
        // In a real scenario, these would come through the network and be validated
        qbft_instance.round_change_container.add_message(
            2.into(),                      // round
            OperatorId::from(operator_id), // sender
            &wrapped_rc,
        );
    }

    // Clear any messages that might have been sent during setup
    sent_messages.lock().unwrap().clear();

    // Now the critical test: trigger the leader proposal logic
    // This should call justify_round_change_quorum() which should return
    // RcJustificationOutcome::PreparedExistsButDataMissing because:
    // 1. There IS a quorum of round change messages (3 messages)
    // 2. They claim the highest prepared data (prepared_hash from round 1)
    // 3. But the full_data for that hash is missing from our data map

    // Simulate what happens when the leader tries to propose
    // This is equivalent to what start_round() does when we're the leader
    if qbft_instance.config.leader_fn().leader_function(
        &qbft_instance.config.operator_id(),
        qbft_instance.current_round,
        qbft_instance.instance_height,
        qbft_instance.config.committee_members(),
    ) {
        // This is the core logic being tested - the justify_round_change_quorum call
        let justification_outcome = qbft_instance.justify_round_change_quorum();

        match justification_outcome {
            RcJustificationOutcome::HighestPrepared(_) => {
                panic!("BUG: Should not have highest prepared data since full_data is missing!");
            }
            RcJustificationOutcome::NoPrepared => {
                panic!("BUG: Should detect prepared data exists (even though missing)!");
            }
            RcJustificationOutcome::PreparedExistsButDataMissing(hash) => {
                assert_eq!(
                    hash, prepared_hash,
                    "Should detect the correct missing hash"
                );
                // Leader should NOT propose - this is the correct behavior
            }
        }
    } else {
        panic!("Test setup error: Node 2 should be leader for round 2");
    }

    // Verify that NO proposal was sent
    let messages = sent_messages.lock().unwrap();
    let proposal_count = messages
        .iter()
        .filter(|msg| {
            matches!(
                msg.qbft_message.qbft_message_type,
                QbftMessageType::Proposal
            )
        })
        .count();

    assert_eq!(
        proposal_count, 0,
        "BUG: Leader sent {} proposal(s) when highest prepared data is missing! \
         The RcJustificationOutcome::PreparedExistsButDataMissing case should cause \
         the leader to wait instead of proposing.",
        proposal_count
    );

    // Test passes if we reach this point without panicking
}
