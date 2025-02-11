//! A collection of unit tests for the QBFT Protocol.
//!
//! These test individual components and also provide full end-to-end tests of the entire protocol.

use super::*;
use qbft_types::DefaultLeaderFunction;
use sha2::{Digest, Sha256};
use ssv_types::consensus::UnsignedSSVMessage;
use ssv_types::message::SignedSSVMessage;
use ssv_types::OperatorId;
use ssz::{Decode, DecodeError, Encode};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use tracing_subscriber::filter::EnvFilter;
use types::Hash256;

// HELPER FUNCTIONS FOR TESTS

/// Enable debug logging for tests
const ENABLE_TEST_LOGGING: bool = true;

/// Test data structure that implements the Data trait
#[derive(Debug, Clone, Default)]
struct TestData(u64);

impl Encode for TestData {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let value = self.0;
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn ssz_fixed_len() -> usize {
        8 // u64 size
    }

    fn ssz_bytes_len(&self) -> usize {
        8 // u64 size
    }
}

impl Decode for TestData {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        8 // u64 size
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != 8 {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: 8,
            });
        }
        let value = u64::from_le_bytes(bytes.try_into().unwrap());
        Ok(TestData(value))
    }
}

impl QbftData for TestData {
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

fn convert_unsigned_to_wrapped(
    msg: UnsignedSSVMessage,
    operator_id: OperatorId,
) -> WrappedQbftMessage {
    // Create a signed message containing just this operator
    let signed_message = SignedSSVMessage::new(
        vec![vec![0; 96]], // Test signature of 96 bytes
        vec![*operator_id],
        msg.ssv_message.clone(),
        msg.full_data,
    )
    .expect("Should create signed message");

    // Parse the QBFT message from the SSV message data
    let qbft_message =
        QbftMessage::from_ssz_bytes(msg.ssv_message.data()).expect("Should decode QBFT message");

    WrappedQbftMessage {
        signed_message,
        qbft_message,
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
    pub fn run<D>(self, data: D) -> TestQBFTCommittee<D, impl FnMut(Message)>
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
struct TestQBFTCommittee<D: QbftData<Hash = Hash256>, S: FnMut(Message)> {
    msg_queue: Rc<RefCell<VecDeque<(OperatorId, Message)>>>,
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
) -> TestQBFTCommittee<D, impl FnMut(Message)> {
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

impl<D: QbftData<Hash = Hash256>, S: FnMut(Message)> TestQBFTCommittee<D, S> {
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
                // We do not make sure that id != sender since we want to loop back and receive our
                // own messages
                let instance = self.instances.get_mut(id).expect("Instance exists");
                // get the unsigned message and the sender
                let (_, unsigned) = match msg {
                    Message::Propose(o, ref u)
                    | Message::Prepare(o, ref u)
                    | Message::Commit(o, ref u)
                    | Message::RoundChange(o, ref u) => (o, u),
                };

                let wrapped = convert_unsigned_to_wrapped(unsigned.clone(), sender);
                instance.receive(wrapped);
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
