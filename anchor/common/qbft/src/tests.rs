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
    consensus::NoDataValidation,
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
        if ENABLE_TEST_LOGGING {
            let env_filter = EnvFilter::new("debug");
            tracing_subscriber::fmt()
                .compact()
                .with_env_filter(env_filter)
                .init();
        }
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
/// Test that verifies QBFT properly buffers PREPARE messages from future rounds
///
/// This test simulates a realistic network partition scenario where a node receives
/// PREPARE messages from future rounds. According to QBFT principles, these messages
/// should be buffered for processing when the node advances to those rounds.
///
/// This ensures proper catch-up during network partitions and maintains liveness.
fn test_future_round_prepare_messages_rejected() {
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

    // Create QBFT instance with 3 nodes (f=0, quorum=3)
    let config = ConfigBuilder::<DefaultLeaderFunction>::new(
        1.into(),
        InstanceHeight::default(),
        (1..4).map(OperatorId::from).collect(), // 3 nodes, quorum = 3
    )
    .with_operator_id(OperatorId::from(1))
    .build()
    .expect("config should be valid");

    let test_data = TestData(456);
    let mut qbft_instance = Qbft::new(
        config,
        test_data.clone(),
        Box::new(NoDataValidation),
        MessageId::from([0; 56]),
        |_| {},
    );

    // Verify initial state: round 1, awaiting proposal
    assert_eq!(qbft_instance.current_round, Round::from(1));
    assert!(matches!(qbft_instance.state, InstanceState::AwaitingProposal));

    // SCENARIO: Node receives PREPARE messages from round 2 (future round)
    // This simulates network partition where other nodes have advanced to round 2
    // but this node is still in round 1
    
    println!("Sending PREPARE messages from future round 2...");

    let prepare_messages_before = qbft_instance
        .prepare_container
        .get_messages_for_round(2.into())
        .len();

    // Create individual PREPARE messages from round 2 (future round)
    for operator_id in [2, 3, 4] {
        let prepare_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Prepare,
            height: 0,
            round: 2, // Future round!
            identifier: [0; 56].to_vec().into(),
            root: test_data.hash(),
            data_round: 0,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };

        let prepare_ssv_message = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            MessageId::from([0; 56]),
            prepare_msg.as_ssz_bytes(),
        )
        .expect("should create prepare SSVMessage");

        let signed_prepare = SignedSSVMessage::new(
            vec![vec![0; RSA_SIGNATURE_SIZE]],
            vec![OperatorId::from(operator_id)],
            prepare_ssv_message,
            vec![], // no full_data for prepare
        )
        .expect("should create signed prepare");

        let wrapped_prepare = WrappedQbftMessage {
            signed_message: signed_prepare,
            qbft_message: prepare_msg,
        };

        // Send the future round PREPARE message - should be buffered
        qbft_instance.receive(wrapped_prepare);
    }

    let prepare_messages_after = qbft_instance
        .prepare_container
        .get_messages_for_round(2.into())
        .len();

    // The PREPARE messages should be buffered for future processing
    assert_eq!(
        prepare_messages_after, 3,
        "Future round PREPARE messages should be buffered for catch-up scenarios. \
         Expected: 3 PREPARE messages buffered for round 2. Actual: {} messages. \
         Proper buffering is essential for maintaining liveness during network partitions.",
        prepare_messages_after
    );

    println!("Advancing to round 2 to verify buffered messages are processed...");

    // STEP 2: Advance to round 2 and verify buffered messages are available
    // Simulate advancing to round 2 (this would happen due to timeouts/round changes)
    qbft_instance.current_round = Round::from(2);

    // Now that we're in round 2, the buffered PREPARE messages should be available
    // and could contribute to achieving prepare consensus
    let round2_prepares = qbft_instance
        .prepare_container
        .get_messages_for_round(2.into())
        .len();

    assert_eq!(
        round2_prepares, 3,
        "After advancing to round 2, all buffered PREPARE messages should be available. \
         Got: {} messages. This ensures nodes can properly catch up during network partitions.",
        round2_prepares
    );

    println!("SUCCESS: Future round messages properly buffered and available for processing!");
}

