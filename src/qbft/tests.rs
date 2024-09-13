//! A collection of unit tests for the QBFT Protocol.
//!
//! These test individual components and also provide full end-to-end tests of the entire protocol.

use super::*;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tracing::{debug, instrument};
use tracing_subscriber;

// HELPER FUNCTIONS FOR TESTS

/// A struct to help build and initialise a test of running instances
struct TestQBFTCommitteeBuilder {
    /// The size of the test committee. (Default is 5).
    committee_size: usize,
    /// The configuration to use for all the instances.
    config: Config,
    /// Whether we should send back dummy validation input to each instance when it requests it.
    emulate_validation: bool,
    /// Whether to emulate a broadcast network and have all network-related messages be relayed to
    /// teach instance.
    emulate_broadcast_network: bool,
    /// Whether to emit logs during testing or not.
    emit_logs: bool,
}

impl Default for TestQBFTCommitteeBuilder {
    fn default() -> Self {
        TestQBFTCommitteeBuilder {
            committee_size: 5,
            config: Config::default(),
            emulate_validation: true,
            emulate_broadcast_network: true,
            emit_logs: false,
        }
    }
}

#[allow(dead_code)]
impl TestQBFTCommitteeBuilder {
    /// Sets the size of the testing committee.
    pub fn committee_size(mut self, commitee_size: usize) -> Self {
        self.committee_size = commitee_size;
        self
    }

    /// Set whether to emulate validation or not
    pub fn emulate_validation(mut self, emulate: bool) -> Self {
        self.emulate_validation = emulate;
        self
    }
    /// Set whether to emulate network or not
    pub fn emulate_broadcast_network(mut self, emulate: bool) -> Self {
        self.emulate_broadcast_network = emulate;
        self
    }

    /// Sets the config for all instances to run
    pub fn set_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Will emit logs during the test running
    pub fn emit_logs(mut self) -> Self {
        self.emit_logs = true;
        self
    }

    /// Consumes self and runs a test scenario. This returns a [`TestQBFTCommittee`] which
    /// represents a running quorum.
    pub fn run(self) -> TestQBFTCommittee {
        if self.emit_logs {
            tracing_subscriber::fmt::init();
        }

        let (senders, mut receivers) =
            construct_and_run_committee(self.config, self.committee_size);

        if self.emulate_validation {
            receivers = emulate_validation(receivers, senders.clone());
        }

        if self.emulate_broadcast_network {
            receivers = emulate_broadcast_network(receivers, senders.clone());
        }

        TestQBFTCommittee { senders, receivers }
    }
}

/// A testing structure representing a committee of running instances
struct TestQBFTCommittee {
    /// Channels to receive all the messages coming out of all the running qbft instances
    receivers: Vec<UnboundedReceiver<OutMessage>>,
    /// Channels to send messages to all the running qbft instances    
    senders: Vec<UnboundedSender<InMessage>>,
}

#[allow(dead_code)]
impl TestQBFTCommittee {
    /// Waits until all the instances have ended
    pub async fn wait_until_end(&mut self) {
        // Loops through and waits for messages from all channels until there is nothing left.
        while let Some(_) = self
            .receivers
            .iter_mut()
            .map(|recv| recv.recv())
            .collect::<FuturesUnordered<_>>()
            .next()
            .await
        {}
    }

    /// Sends a message to an instance. Specify its index (or id) and the message you want to send.
    pub fn send_message(&mut self, instance: usize, message: InMessage) {
        let _ = self.senders[instance].send(message);
    }
}

/// Constructs and runs committee of QBFT Instances
///
/// This will create instances and spawn them in a task and return the sender/receiver channels for
/// all created instances.
#[instrument]
fn construct_and_run_committee(
    config: Config,
    committee_size: usize,
) -> (
    Vec<UnboundedSender<InMessage>>,
    Vec<UnboundedReceiver<OutMessage>>,
) {
    // The ID of a committee is just an integer in [0,committee_size)

    // A collection of channels to send messages to each instance.
    let mut senders = Vec::new();
    // A collection of channels to receive messages from each instances.
    // We will redirect messages to each instance, simulating a broadcast network.
    let mut receivers = Vec::new();

    for id in 0..committee_size {
        // Creates a new instance
        // TODO: Will need to define an ID
        let (sender, receiver, instance) = Qbft::new(config.clone());
        senders.push(sender);
        receivers.push(receiver);

        // spawn the instance
        // TODO: Make the round time adjustable, to get deterministic results for testing.
        debug!(id, "Starting instance");
        tokio::spawn(instance.start_instance());
    }

    (senders, receivers)
}

/// This will collect all the outbound messages that are destined for local not network
/// interaction.
///
/// Specifically it handles:
/// - GetData
/// - Validate
///
/// It will respond to these messages back to the instance that requested them with arbitrary data.
/// In order to just respond to these messages and forward others on, this function takes ownership
/// of the receive channels and replaces them with new ones in the return value. The sending
/// channel can be cloned and put in here.
///
/// We duplicate the messages that we consume, so the returned receive channels behave identically
/// to the ones we take ownership of.
#[instrument]
fn emulate_validation(
    receivers: Vec<UnboundedReceiver<OutMessage>>,
    senders: Vec<UnboundedSender<InMessage>>,
) -> Vec<UnboundedReceiver<OutMessage>> {
    debug!("Emulating validation");
    let handle_out_messages_fn =
        |message: OutMessage,
         index: usize,
         senders: &mut Vec<UnboundedSender<InMessage>>,
         new_senders: &mut Vec<UnboundedSender<OutMessage>>| {
            // Duplicate the message to the new channel
            let _ = new_senders[index].send(message.clone());

            match message {
                OutMessage::GetData(_data) => {
                    let _ = senders[index].send(InMessage::RecvData(GetData { value: Vec::new() }));
                }
                OutMessage::Validate(_validation_message) => {
                    let _ = senders[index].send(InMessage::Validate(ValidationMessage {
                        value: Vec::new(),
                        id: 0,
                        round: 0,
                    }));
                }
                // We don't interact with any of the others
                _ => {}
            };
        };

    // Get messages from each instance, apply the function above and return the resulting channels
    generically_handle_messages(receivers, senders, handle_out_messages_fn)
}

/// This function takes the senders and receivers and will duplicate messages from all instances
/// and send those messages to all other instances.
/// This simulates a kind of broadcast network.
/// Specifically it handles:
/// ProposeMessage
/// PrepareMessage
/// ConfirmMessage
/// RoundChange
/// And forwards the others untouched.
#[instrument]
fn emulate_broadcast_network(
    receivers: Vec<UnboundedReceiver<OutMessage>>,
    senders: Vec<UnboundedSender<InMessage>>,
) -> Vec<UnboundedReceiver<OutMessage>> {
    debug!("Emulating a gossip network");
    let emulate_gossip_network_fn =
        |message: OutMessage,
         index: usize,
         senders: &mut Vec<UnboundedSender<InMessage>>,
         new_senders: &mut Vec<UnboundedSender<OutMessage>>| {
            // Duplicate the message
            let _ = new_senders[index].send(message.clone());

            match message {
                OutMessage::Propose(propose_message) => {
                    // Send the message to all other nodes
                    senders
                        .iter_mut()
                        .enumerate()
                        .for_each(|(current_index, sender)| {
                            if current_index != index {
                                let _ = sender.send(InMessage::Propose(propose_message.clone()));
                            }
                        });
                }
                OutMessage::Prepare(prepare_message) => {
                    senders
                        .iter_mut()
                        .enumerate()
                        .for_each(|(current_index, sender)| {
                            if current_index != index {
                                let _ = sender.send(InMessage::Prepare(prepare_message.clone()));
                            }
                        });
                }
                OutMessage::Confirm(confirm_message) => {
                    senders
                        .iter_mut()
                        .enumerate()
                        .for_each(|(current_index, sender)| {
                            if current_index != index {
                                let _ = sender.send(InMessage::Confirm(confirm_message.clone()));
                            }
                        });
                }
                OutMessage::RoundChange(round_change) => {
                    senders
                        .iter_mut()
                        .enumerate()
                        .for_each(|(current_index, sender)| {
                            if current_index != index {
                                let _ = sender.send(InMessage::RoundChange(round_change.clone()));
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
fn generically_handle_messages<T>(
    mut receivers: Vec<UnboundedReceiver<OutMessage>>,
    mut senders: Vec<UnboundedSender<InMessage>>,
    // This is a function that takes the outbound message from the instances and the old inbound
    // sending channel and the new inbound sending channel. Given the outbound message, we can send a
    // response to the old inbound sender, and potentially duplicate the message to the new receiver
    // via the second Sender<OutMessage>.
    mut message_handling: T,
) -> Vec<UnboundedReceiver<OutMessage>>
where
    T: FnMut(
            OutMessage,
            usize,
            &mut Vec<UnboundedSender<InMessage>>,
            &mut Vec<UnboundedSender<OutMessage>>,
        ) -> ()
        + 'static
        + Send
        + Sync,
{
    // Build a new set of channels to replace the ones we have taken ownership of. We will just
    // forward network messages to these channels
    let mut new_receivers = Vec::new();
    let mut new_senders = Vec::new();

    // Populate the new channels.
    for _ in 0..receivers.len() {
        let (new_sender, new_receiver) = tokio::sync::mpsc::unbounded_channel::<OutMessage>();
        new_receivers.push(new_receiver);
        new_senders.push(new_sender);
    }

    // Run a task to handle all the out messages

    tokio::spawn(async move {
        loop {
            // First need to group all the receive channels into a single Stream that we can await.
            // We will use a FuturesUnordered which groups a collection of futures.
            // We also need to know the number of which receiver sent us the message so we know
            // which sender to forward to. For this reason we make a little intermediate type with the
            // index.
            let mut grouped_receive: FuturesUnordered<_> = receivers
                .iter_mut()
                .enumerate()
                .map(|(index, recv)| async move { (index, recv.recv().await) })
                .collect();

            match grouped_receive.next().await {
                Some((index, Some(out_message))) => {
                    debug!(
                        ?out_message,
                        "Instance" = index,
                        "Handling message from instance"
                    );
                    // Custom handling of the out message
                    message_handling(out_message, index, &mut senders, &mut new_senders);
                }
                Some((index, None)) => {
                    // The instance has shutdown
                    debug!(index, "Instance shutdown");
                    // Remove the channel from the futures list
                    // Note that we need to drop here, because we are holding a mutable reference
                    // to receivers, so cannot remove until we drop the borrow.
                    drop(grouped_receive);
                    receivers.remove(index);
                }
                None => {
                    // At least one instance has finished.
                    debug!("Ending handling messages");
                    break;
                }
            }
        }
    });

    // Return the channels that will just handle network messages
    new_receivers
}

#[tokio::test]
async fn test_basic_committee() {
    // Construct and run a test committee
    let mut test_instance = TestQBFTCommitteeBuilder::default().emit_logs().run();

    // Wait until consensus is reached or all the instances have ended
    test_instance.wait_until_end().await;
}
