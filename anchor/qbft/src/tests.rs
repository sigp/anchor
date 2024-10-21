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
use tracing::debug;
use tracing_subscriber::filter::EnvFilter;
use types::DefaultLeaderFunction;

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

        let (senders, mut receivers) = construct_and_run_committee(self.config, validated_data);

        if self.emulate_broadcast_network {
            receivers = emulate_broadcast_network(receivers, senders.clone());
        }

        TestQBFTCommittee { senders, receivers }
    }
}

/// A testing structure representing a committee of running instances
#[allow(dead_code)]
struct TestQBFTCommittee<D: Default + Clone + Debug + Send + Sync + 'static + Eq + Hash> {
    /// Channels to receive all the messages coming out of all the running qbft instances
    receivers: HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>>,
    /// Channels to send messages to all the running qbft instances
    senders: HashMap<OperatorId, UnboundedSender<InMessage<D>>>,
}

impl<D> TestQBFTCommittee<D>
where
    D: Debug + Default + Clone + Send + Sync + 'static + Eq + Hash,
{
    /// Waits until all the instances have ended
    pub async fn wait_until_end(&mut self) {
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
        while all_recievers.next().await.is_some() {}
        debug!("Completed");
    }

    /// Sends a message to an instance. Specify its index (or id) and the message you want to send.
    #[allow(dead_code)]
    pub fn send_message(&mut self, operator_id: &OperatorId, message: InMessage<D>) {
        let _ = self.senders.get(operator_id).unwrap().send(message);
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
) {
    // The ID of a committee is just an integer in [0,committee_size)

    // A collection of channels to send messages to each instance.
    let mut senders = HashMap::with_capacity(config.committee_size);
    // A collection of channels to receive messages from each instances.
    // We will redirect messages to each instance, simulating a broadcast network.
    let mut receivers = HashMap::with_capacity(config.committee_size);

    for id in 0..config.committee_size {
        // Creates a new instance
        config.operator_id = OperatorId::from(id);
        let (sender, receiver, instance) = Qbft::new(config.clone(), validated_data.clone());
        senders.insert(config.operator_id, sender);
        receivers.insert(config.operator_id, receiver);

        // spawn the instance
        debug!(id, "Starting instance");
        tokio::spawn(instance.start_instance());
    }

    (senders, receivers)
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
) -> HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>> {
    debug!("Emulating a gossip network");
    let emulate_gossip_network_fn =
        |message: OutMessage<D>,
         operator_id: &OperatorId,
         senders: &mut HashMap<OperatorId, UnboundedSender<InMessage<D>>>,
         new_senders: &mut HashMap<OperatorId, UnboundedSender<OutMessage<D>>>| {
            // Duplicate the message to the new channel
            let _ = new_senders.get(operator_id).unwrap().send(message.clone());

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

    generically_handle_messages(receivers, senders, emulate_gossip_network_fn)
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
) -> HashMap<OperatorId, UnboundedReceiver<OutMessage<D>>>
where
    T: FnMut(
            OutMessage<D>,
            &OperatorId,
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
            /*
                    debug!(
                        ?out_message,
                        operator = *operator_id,
                        "Handling message from instance"
                    );
            */
            // Custom handling of the out message
            message_handling(out_message, &operator_id, &mut senders, &mut new_senders);
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
    test_instance.wait_until_end().await;
}
