//! A collection of unit tests for the QBFT Protocol.
//!
//! These test individual components and also provide full end-to-end tests of the entire protocol.

use super::*;
use crate::validation::{validate_data, ValidatedData};
use futures::stream::select_all;
use futures::StreamExt;
use std::cmp::Eq;
use std::hash::Hash;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::task::JoinHandle;
use tracing::debug;
use tracing_subscriber::filter::EnvFilter;
use types::DefaultLeaderFunction;

use std::sync::Arc;
use std::sync::RwLock;

// HELPER FUNCTIONS FOR TESTS

/// Enable debug logging for tests
const ENABLE_TEST_LOGGING: bool = true;

/// A struct to help build and initialise a test of running instances
struct TestQBFTCommitteeBuilder {
    /// The configuration to use for all the instances.
    config: Config<DefaultLeaderFunction>,
    /// Whether we should send back dummy validation input to each instance when it requests it.
    emulate_client_processor: bool,
    /// Whether to emulate a broadcast network and have all network-related messages be relayed to
    /// teach instance.
    emulate_broadcast_network: bool,
}

impl Default for TestQBFTCommitteeBuilder {
    fn default() -> Self {
        let config = Config::<DefaultLeaderFunction> {
            // Set a default committee size of 5.
            committee_size: 5,
            // Populate the committee members
            committee_members: (0..5).map(OperatorId::from).collect::<HashSet<_>>(),
            ..Default::default()
        };

        TestQBFTCommitteeBuilder {
            config,
            emulate_client_processor: true,
            emulate_broadcast_network: true,
        }
    }
}

#[allow(dead_code)]
impl TestQBFTCommitteeBuilder {
    /// Sets the size of the testing committee.
    pub fn committee_size(mut self, committee_size: usize) -> Self {
        self.config.committee_size = committee_size;
        self
    }

    /// Set whether to emulate validation or not
    pub fn emulate_validation(mut self, emulate: bool) -> Self {
        self.emulate_client_processor = emulate;
        self
    }
    /// Set whether to emulate network or not
    pub fn emulate_broadcast_network(mut self, emulate: bool) -> Self {
        self.emulate_broadcast_network = emulate;
        self
    }

    /// Sets the config for all instances to run
    pub fn set_config(mut self, config: Config<DefaultLeaderFunction>) -> Self {
        self.config = config;
        self
    }

    /// Consumes self and runs a test scenario. This returns a [`TestQBFTCommittee`] which
    /// represents a running quorum.
    pub fn run<D>(self, data: D) -> TestQBFTCommittee<D>
    where
        D: Debug + Default + Clone + Send + Sync + 'static + Eq + Hash,
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

        let (senders, mut receivers, active_instances) =
            construct_and_run_committee(self.config, validated_data);

        let offline_instances: Arc<RwLock<HashSet<OperatorId>>> = Arc::default();
        if self.emulate_broadcast_network {
            receivers =
                emulate_broadcast_network(receivers, senders.clone(), offline_instances.clone());
        }

        TestQBFTCommittee {
            senders,
            receivers,
            offline_instances,
            active_instances,
        }
    }
}

/// A testing structure representing a committee of running instances
#[allow(dead_code)]
struct TestQBFTCommittee<D: Default + Clone + Debug + Send + Sync + 'static + Eq + Hash> {
    /// Channels to receive all the messages coming out of all the running qbft instances
    receivers: HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>>,
    /// Channels to send messages to all the running qbft instances
    senders: HashMap<OperatorId, UnboundedSender<InMessage<D>>>,
    /// Handles to running QBFT instances
    active_instances: HashMap<OperatorId, JoinHandle<()>>,
    /// All of the instances that are offline. This needs to be mutably accessed in the tests and
    /// also in the handler, so we wrap it in a Arc<RwLock>
    offline_instances: Arc<RwLock<HashSet<OperatorId>>>,
}

impl<D> TestQBFTCommittee<D>
where
    D: Debug + Default + Clone + Send + Sync + 'static + Eq + Hash,
{
    /// Waits until all the instances have ended and report the number of nodes that were able to
    /// reach consensus
    pub async fn wait_until_end(&mut self) -> i32 {
        debug!("Waiting for completion");
        // Loops through and waits for messages from all channels until there is nothing left.

        // Cheeky Hack, might need to change in the future
        let receivers = std::mem::take(&mut self.receivers);

        let mut all_recievers =
            select_all(
                receivers
                    .into_iter()
                    .map(|(operator_id, receiver)| InstanceStream::<D> {
                        operator_id,
                        receiver,
                    }),
            );

        // Record the number of members that were able to reach consensus
        // Complete::Success(D) message inidicates successful consensus
        let mut num_consensus = 0;
        while let Some((_, msg)) = all_recievers.next().await {
            if let OutMessage::Completed(Completed::Success(_)) = msg {
                num_consensus += 1;
            }
        }
        num_consensus
    }

    /// Sends a message to an instance. Specify its index (or id) and the message you want to send.
    #[allow(dead_code)]
    pub fn send_message(&mut self, operator_id: &OperatorId, message: InMessage<D>) {
        let _ = self.senders.get(operator_id).unwrap().send(message);
    }

    // Pause an instance, this will block any messages being sent out from this operator to
    // simulate it being offline
    pub fn pause_instance(&mut self, operator_id: &OperatorId) {
        let mut write_guard = self.offline_instances.write().expect("Failed to get write");
        write_guard.insert(*operator_id);
    }

    // Restart an instance after it has been paused. This corresponds to a node coming back online
    pub fn recover_instance(&mut self, operator_id: &OperatorId) {
        let mut write_guard = self.offline_instances.write().expect("Failed to get write");
        write_guard.remove(operator_id);
    }
}

// Helper type to handle Streams with instance ids.
//
// I wanted a Stream that returns the instance id as well as the message when it becomes ready.
// TODO: Can probably group this thing via a MAP in a stream function.
struct InstanceStream<D: Clone + Default + Debug + Eq + Hash> {
    operator_id: OperatorId,
    receiver: UnboundedReceiver<OutMessage<D>>,
}

impl<D> futures::Stream for InstanceStream<D>
where
    D: Debug + Default + Clone + Eq + Hash,
{
    type Item = (OperatorId, OutMessage<D>);

    // Required method
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(message)) => Poll::Ready(Some((self.operator_id, message))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Constructs and runs committee of QBFT Instances
///
/// This will create instances and spawn them in a task and return the sender/receiver channels for
/// all created instances.
#[allow(clippy::type_complexity)]
fn construct_and_run_committee<D: Debug + Default + Clone + Send + Sync + 'static + Eq + Hash>(
    mut config: Config<DefaultLeaderFunction>,
    validated_data: ValidatedData<D>,
) -> (
    HashMap<OperatorId, UnboundedSender<InMessage<D>>>,
    HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>>,
    HashMap<OperatorId, JoinHandle<()>>,
) {
    // The ID of a committee is just an integer in [0,committee_size)

    // A collection of channels to send messages to each instance.
    let mut senders = HashMap::with_capacity(config.committee_size);
    // A collection of channels to receive messages from each instances.
    // We will redirect messages to each instance, simulating a broadcast network.
    let mut receivers = HashMap::with_capacity(config.committee_size);
    // A collection of handles to active QBFT instances
    let mut handles = HashMap::with_capacity(config.committee_size);

    for id in 0..config.committee_size {
        // Creates a new instance
        config.operator_id = OperatorId::from(id);
        let (sender, receiver, instance) = Qbft::new(config.clone(), validated_data.clone());
        senders.insert(config.operator_id, sender);
        receivers.insert(config.operator_id, receiver);

        // spawn the instance
        debug!(id, "Starting instance");
        let handle = tokio::spawn(instance.start_instance());
        handles.insert(config.operator_id, handle);
    }

    (senders, receivers, handles)
}

/// This function takes the senders and receivers and will duplicate messages from all instances
/// and send those messages to all other instances.
/// This simulates a kind of broadcast network.
/// Specifically it handles:
/// ProposeMessage
/// PrepareMessage
/// CommitMessage
/// RoundChange
/// And forwards the others untouched.
fn emulate_broadcast_network<D: Default + Debug + Clone + Send + Sync + 'static + Eq + Hash>(
    receivers: HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>>,
    senders: HashMap<OperatorId, UnboundedSender<InMessage<D>>>,
    offline_instances: Arc<RwLock<HashSet<OperatorId>>>,
) -> HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>> {
    debug!("Emulating a gossip network");
    let emulate_gossip_network_fn =
        |message: OutMessage<D>,
         operator_id: &OperatorId,
         offline_instances: Arc<RwLock<HashSet<OperatorId>>>,
         senders: &mut HashMap<OperatorId, UnboundedSender<InMessage<D>>>,
         new_senders: &mut HashMap<OperatorId, UnboundedSender<OutMessage<D>>>| {
            // Duplicate the message to the new channel
            let _ = new_senders.get(operator_id).unwrap().send(message.clone());

            // if we have paused this instance, just ignore any messages it has to send
            // this simulates the node being down
            let read_guard = offline_instances
                .read()
                .expect("Failed to acquire read lock");
            if read_guard.contains(operator_id) {
                return;
            }

            match message {
                OutMessage::Propose(consensus_data) => {
                    // Send the message to all other nodes
                    senders
                        .iter_mut()
                        .for_each(|(current_operator_id, sender)| {
                            if current_operator_id != operator_id {
                                let _ = sender
                                    .send(InMessage::Propose(*operator_id, consensus_data.clone()));
                            }
                        });
                }
                OutMessage::Prepare(prepare_message) => {
                    senders
                        .iter_mut()
                        .for_each(|(current_operator_id, sender)| {
                            if current_operator_id != operator_id {
                                let _ = sender.send(InMessage::Prepare(
                                    *operator_id,
                                    prepare_message.clone(),
                                ));
                            }
                        });
                }
                OutMessage::Commit(commit_message) => {
                    // Ignoring commits in round 2 for testing
                    senders
                        .iter_mut()
                        .for_each(|(current_operator_id, sender)| {
                            if current_operator_id != operator_id {
                                let _ = sender
                                    .send(InMessage::Commit(*operator_id, commit_message.clone()));
                            }
                        })
                }
                OutMessage::RoundChange(round, optional_data) => {
                    senders
                        .iter_mut()
                        .for_each(|(current_operator_id, sender)| {
                            if current_operator_id != operator_id {
                                let _ = sender.send(InMessage::RoundChange(
                                    *operator_id,
                                    round,
                                    optional_data.clone(),
                                ));
                            }
                        });
                }
                _ => {} // We don't interact with any of the others
            };
        };

    generically_handle_messages(
        receivers,
        senders,
        emulate_gossip_network_fn,
        offline_instances,
    )
}

/// This is a base function to prevent duplication of code. It's used by `emulate_gossip_network`
/// and `handle_all_out_messages`. It groups the logic of taking the channels, cloning them and
/// returning new channels. Leaving the logic of message handling as a parameter.
fn generically_handle_messages<T, D: Debug + Default + Clone + Send + Sync + 'static + Eq + Hash>(
    receivers: HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>>,
    mut senders: HashMap<OperatorId, UnboundedSender<InMessage<D>>>,
    // This is a function that takes the outbound message from the instances and the old inbound
    // sending channel and the new inbound sending channel. Given the outbound message, we can send a
    // response to the old inbound sender, and potentially duplicate the message to the new receiver
    // via the second Sender<OutMessage>.
    mut message_handling: T,
    offline_instances: Arc<RwLock<HashSet<OperatorId>>>,
) -> HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>>
where
    T: FnMut(
            OutMessage<D>,
            &OperatorId,
            Arc<RwLock<HashSet<OperatorId>>>,
            &mut HashMap<OperatorId, UnboundedSender<InMessage<D>>>,
            &mut HashMap<OperatorId, UnboundedSender<OutMessage<D>>>,
        )
        + 'static
        + Send
        + Sync,
{
    // Build a new set of channels to replace the ones we have taken ownership of. We will just
    // forward network messages to these channels
    let mut new_receivers = HashMap::with_capacity(receivers.len());
    let mut new_senders = HashMap::with_capacity(senders.len());

    // Populate the new channels.
    for operator_id in receivers.keys() {
        let (new_sender, new_receiver) = tokio::sync::mpsc::unbounded_channel::<OutMessage<D>>();
        new_receivers.insert(*operator_id, new_receiver);
        new_senders.insert(*operator_id, new_sender);
    }

    // Run a task to handle all the out messages

    tokio::spawn(async move {
        // First need to group all the receive channels into a single Stream that we can await.
        // We will use a FuturesUnordered which groups a collection of futures.
        // We also need to know the number of which receiver sent us the message so we know
        // which sender to forward to. For this reason we make a little intermediate type with the
        // index.

        let mut grouped_receivers = select_all(receivers.into_iter().map(
            |(operator_id, receiver)| InstanceStream {
                operator_id,
                receiver,
            },
        ));

        while let Some((operator_id, out_message)) = grouped_receivers.next().await {
            debug!(
                ?out_message,
                operator = *operator_id,
                "Handling message from instance"
            );
            // Custom handling of the out message
            message_handling(
                out_message,
                &operator_id,
                offline_instances.clone(),
                &mut senders,
                &mut new_senders,
            );
            // Add back a new future to await for the next message
        }

        /* loop {
            match grouped_receivers.next().await {
                Some((index, out_message)) => {
                    debug!(
                        ?out_message,
                        "Instance" = index,
                        "Handling message from instance"
                    );
                    // Custom handling of the out message
                    message_handling(out_message, index, &mut senders, &mut new_senders);
                    // Add back a new future to await for the next message
                }
                None => {
                    // At least one instance has finished.
                    break;
                }
            }
        }*/
        debug!("Task shutdown");
    });

    // Return the channels that will just handle network messages
    new_receivers
}

#[tokio::test]
async fn test_basic_committee() {
    // Construct and run a test committee

    let mut test_instance = TestQBFTCommitteeBuilder::default().run(21);

    // Wait until consensus is reached or all the instances have ended
    let num_consensus = test_instance.wait_until_end().await;
    assert!(num_consensus == 5);
}

#[tokio::test]
// Test consensus recovery with F faulty operators
async fn test_consensus_with_f_faulty_operators() {
    let committee_size = 7; // This will allow for F=2 faulty operators
    let mut test_instance = TestQBFTCommitteeBuilder::default()
        .committee_size(committee_size)
        .run(42);

    // Try to simulate faulty behavior by having two operators (=F) stop participating
    test_instance
        .active_instances
        .get(&OperatorId::from(4))
        .unwrap()
        .abort();
    test_instance
        .active_instances
        .get(&OperatorId::from(6))
        .unwrap()
        .abort();

    // System should still reach consensus
    let num_consensus = test_instance.wait_until_end().await;
    assert!(num_consensus == 5);
}

#[tokio::test]
// Test consensus failure when faulty > F
async fn test_consensus_failure() {
    let committee_size = 5;
    let mut test_instance = TestQBFTCommitteeBuilder::default()
        .committee_size(committee_size)
        .run(10);

    // Try to simulate consensus failure by stoping > F instances
    test_instance
        .active_instances
        .get(&OperatorId::from(2))
        .unwrap()
        .abort();
    test_instance
        .active_instances
        .get(&OperatorId::from(4))
        .unwrap()
        .abort();

    // System should not reach consensus
    let num_consensus = test_instance.wait_until_end().await;
    assert!(num_consensus == 0);
}

#[tokio::test]
// Test handling of an invalid proposal
async fn test_invalid_proposal() {
    let committee_size = 5;
    let mut test_instance = TestQBFTCommitteeBuilder::default()
        .committee_size(committee_size)
        .run(42);

    // Inject proposal from a node that is not the leader.
    let proposal2 = ConsensusData {
        round: Round(0),
        data: 24,
    };
    for id in 0..5 {
        test_instance.send_message(
            &OperatorId::from(id),
            InMessage::Propose(OperatorId::from(id), proposal2.clone()),
        );
    }

    // Should still reach consensus on the initial valid proposal
    let num_consensus = test_instance.wait_until_end().await;
    assert!(num_consensus == 5);
}

#[tokio::test]
// Test resistance to message replay attacks
async fn test_message_replay() {
    let committee_size = 5;
    let mut test_instance = TestQBFTCommitteeBuilder::default()
        .committee_size(committee_size)
        .run(42);

    // Initial valid prepare message
    let prepare_msg = ConsensusData {
        round: Round(0),
        data: 42,
    };

    // Replay same prepare message multiple times
    for _ in 0..3 {
        test_instance.send_message(
            &OperatorId::from(0),
            InMessage::Prepare(OperatorId::from(0), prepare_msg.clone()),
        );
    }

    // Should ignore duplicates and still reach consensus
    let num_consensus = test_instance.wait_until_end().await;
    assert!(num_consensus == 5);
}

#[tokio::test]
// Test recovery after round timeouts
async fn test_round_timeout_recovery() {
    let committee_size = 5;
    let mut test_instance = TestQBFTCommitteeBuilder::default()
        .committee_size(committee_size)
        .run(42);

    // Remove the leader right away, this should trigger a round change
    test_instance
        .active_instances
        .get(&OperatorId::from(0))
        .unwrap()
        .abort();

    // Should still reach consensus eventually
    let num_consensus = test_instance.wait_until_end().await;
    assert!(num_consensus > 0);
}

#[tokio::test]
// Test starting with > F offline nodes and then recovering them
async fn test_node_recovery() {
    let committee_size = 5;
    let mut test_instance = TestQBFTCommitteeBuilder::default()
        .committee_size(committee_size)
        .run(42);

    // Pause both instances, consensus should no longer be able to make progress
    test_instance.pause_instance(&OperatorId::from(2));
    test_instance.pause_instance(&OperatorId::from(3));

    // sleep for a few rounds
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    // Recover the instances, we should now reach consensus
    test_instance.recover_instance(&OperatorId::from(2));
    test_instance.recover_instance(&OperatorId::from(3));

    // Since we brought both of the operators back online, we should have reached consensus
    let num_consensus = test_instance.wait_until_end().await;
    assert!(num_consensus == 5);
}

#[tokio::test]
// Test sending a random invalid message
async fn test_invalid_message() {
    let committee_size = 5;
    let mut test_instance = TestQBFTCommitteeBuilder::default()
        .committee_size(committee_size)
        .run(42);

    // Try sending invalid, out of order message
    let future_round_msg = ConsensusData {
        round: Round(5), // Future round
        data: 24,
    };

    test_instance.send_message(
        &OperatorId::from(0),
        InMessage::Prepare(OperatorId::from(0), future_round_msg),
    );

    let num_consensus = test_instance.wait_until_end().await;
    assert!(num_consensus == 5);
}
