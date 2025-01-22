//! A collection of unit tests for the QBFT Protocol.
//!
//! These test individual components and also provide full end-to-end tests of the entire protocol.

use super::*;
use crate::validation::{validate_data, ValidatedData};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use tracing_subscriber::filter::EnvFilter;
use types::DefaultLeaderFunction;

// HELPER FUNCTIONS FOR TESTS

/// Enable debug logging for tests
const ENABLE_TEST_LOGGING: bool = true;

/// A struct to help build and initialise a test of running instances
struct TestQBFTCommitteeBuilder {
    /// The configuration to use for all the instances.
    config: ConfigBuilder,
}

impl Default for TestQBFTCommitteeBuilder {
    fn default() -> Self {
        TestQBFTCommitteeBuilder {
            config: ConfigBuilder::new(
                0.into(),
                InstanceHeight::default(),
                (0..5).map(OperatorId::from).collect(),
            ),
        }
    }
}

#[allow(dead_code)]
impl TestQBFTCommitteeBuilder {
    /// Consumes self and runs a test scenario. This returns a [`TestQBFTCommittee`] which
    /// represents a running quorum.
    pub fn run<D>(self, data: D) -> TestQBFTCommittee<D, impl FnMut(Message<D>)>
    where
        D: Default + Data,
    {
        if ENABLE_TEST_LOGGING {
            let env_filter = EnvFilter::new("debug");
            tracing_subscriber::fmt()
                .compact()
                .with_env_filter(env_filter)
                .init();
        }

        // Validate the data
        let validated_data = validate_data(data).unwrap();

        construct_and_run_committee(self.config, validated_data)
    }
}

/// A testing structure representing a committee of running instances
#[allow(clippy::type_complexity)]
struct TestQBFTCommittee<D: Default + Data + 'static, S: FnMut(Message<D>)> {
    msg_queue: Rc<RefCell<VecDeque<(OperatorId, Message<D>)>>>,
    instances: HashMap<OperatorId, Qbft<DefaultLeaderFunction, D, S>>,
    // All of the instances that are currently active, allows us to stop/restart instances by
    // controlling the messages being send and received
    active_instances: HashSet<OperatorId>,
}

/// Constructs and runs committee of QBFT Instances
///
/// This will create instances and spawn them in a task and return the sender/receiver channels for
/// all created instances.
fn construct_and_run_committee<D: Data + Default + 'static>(
    mut config: ConfigBuilder,
    validated_data: ValidatedData<D>,
) -> TestQBFTCommittee<D, impl FnMut(Message<D>)> {
    // The ID of a committee is just an integer in [0,committee_size)

    let msg_queue = Rc::new(RefCell::new(VecDeque::new()));
    let mut instances = HashMap::with_capacity(config.committee_members().len());
    let mut active_instances = HashSet::new();

    for id in 0..config.committee_members().len() {
        let msg_queue = Rc::clone(&msg_queue);
        let id = OperatorId::from(id);
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

impl<D: Default + Data, S: FnMut(Message<D>)> TestQBFTCommittee<D, S> {
    fn wait_until_end(mut self) -> i32 {
        loop {
            let msg = self.msg_queue.borrow_mut().pop_front();
            let Some((sender, msg)) = msg else {
                // we are done! check how many instances reached consensus
                let mut num_consensus = 0;
                for id in self.active_instances.iter() {
                    let instance = self.instances.get_mut(id).expect("Instance exists");
                    // Check if this instance just reached consensus
                    if matches!(instance.completed(), Some(Completed::Success(_))) {
                        num_consensus += 1;
                    }
                }
                return num_consensus;
            };

            // Only recieve messages for active instances
            for id in self.active_instances.iter() {
                if *id != sender {
                    let instance = self.instances.get_mut(id).expect("Instance exists");
                    instance.receive(msg.clone());
                }
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

#[derive(Debug, Copy, Clone, Default)]
struct TestData(usize);

impl Data for TestData {
    type Hash = usize;

    fn hash(&self) -> Self::Hash {
        self.0
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
    test_instance.pause_instance(&OperatorId::from(0));

    // Then restart it
    test_instance.restart_instance(&OperatorId::from(0));

    let num_consensus = test_instance.wait_until_end();
    assert_eq!(num_consensus, 5); // Should reach full consensus after recovery
}

#[test]
fn test_duplicate_proposals() {
    let mut test_instance = TestQBFTCommitteeBuilder::default().run(TestData(42));

    // Send duplicate propose messages
    let msg = Message::Propose(
        OperatorId::from(0),
        ConsensusData {
            round: Round::default(),
            data: TestData(42),
        },
    );

    // Send the same message multiple times
    for id in 0..5 {
        let instance = test_instance
            .instances
            .get_mut(&OperatorId::from(id))
            .unwrap();
        instance.receive(msg.clone());
        instance.receive(msg.clone());
        instance.receive(msg.clone());
    }

    let num_consensus = test_instance.wait_until_end();
    assert_eq!(num_consensus, 5); // Should still reach consensus despite duplicates
}

#[test]
fn test_invalid_sender() {
    let mut test_instance = TestQBFTCommitteeBuilder::default().run(TestData(42));

    // Create a message from an invalid sender (operator id 10 which isn't in the committee)
    let invalid_msg = Message::Propose(
        OperatorId::from(10),
        ConsensusData {
            round: Round::default(),
            data: TestData(42),
        },
    );

    // Send to a valid instance
    let instance = test_instance
        .instances
        .get_mut(&OperatorId::from(0))
        .unwrap();
    instance.receive(invalid_msg);

    let num_consensus = test_instance.wait_until_end();
    assert_eq!(num_consensus, 5); // Should ignore invalid sender and still reach consensus
}

#[test]
fn test_proposal_from_non_leader() {
    let mut test_instance = TestQBFTCommitteeBuilder::default().run(TestData(42));

    // Send proposal from non-leader (node 1)
    let non_leader_msg = Message::Propose(
        OperatorId::from(1),
        ConsensusData {
            round: Round::default(),
            data: TestData(42),
        },
    );

    // Send to all instances
    for instance in test_instance.instances.values_mut() {
        instance.receive(non_leader_msg.clone());
    }

    let num_consensus = test_instance.wait_until_end();
    assert_eq!(num_consensus, 5); // Should ignore non-leader proposal and still reach consensus
}

#[test]
fn test_invalid_round_messages() {
    let mut test_instance = TestQBFTCommitteeBuilder::default().run(TestData(42));

    // Create a message with an invalid round number
    let future_round = Round::default().next().unwrap().next().unwrap(); // Round 3
    let invalid_round_msg = Message::Prepare(
        OperatorId::from(0),
        ConsensusData {
            round: future_round,
            data: 42,
        },
    );

    // Send to all instances
    for instance in test_instance.instances.values_mut() {
        instance.receive(invalid_round_msg.clone());
    }

    let num_consensus = test_instance.wait_until_end();
    assert_eq!(num_consensus, 5); // Should ignore invalid round messages and still reach consensus
}

#[test]
fn test_round_change_timeout() {
    let mut test_instance = TestQBFTCommitteeBuilder::default().run(TestData(42));

    // Pause the leader node to force a round change
    test_instance.pause_instance(&OperatorId::from(0));

    // Manually trigger round changes in all instances
    for instance in test_instance.instances.values_mut() {
        instance.end_round();
    }

    let num_consensus = test_instance.wait_until_end();
    assert_eq!(num_consensus, 4); // Should reach consensus with new leader
}
