//! A collection of unit tests for the QBFT Protocol.
//!
//! These test individual components and also provide full end-to-end tests of the entire protocol.

use super::*;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tracing::debug;

// HELPER FUNCTIONS FOR TESTS

/// Constructs and runs committee of QBFT Instances
///
/// This will create instances and spawn them in a task and return the sender/receiver channels for
/// all created instances.
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
        let (sender, receiver, instance) = QBFT::new(config.clone());
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
fn handle_all_out_messages(
    mut receivers: Vec<UnboundedReceiver<OutMessage>>,
    senders: Vec<UnboundedSender<InMessage>>,
) -> Vec<UnboundedReceiver<OutMessage>> {
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
                    match out_message {
                        OutMessage::GetData(data) => {
                            let _ = senders[index]
                                .send(InMessage::RecvData(GetData { value: Vec::new() }));
                            // Duplicate the message to the new channel
                            let _ = new_senders[index].send(OutMessage::GetData(data));
                        }
                        OutMessage::Validate(validation_message) => {
                            let _ = senders[index].send(InMessage::Validate(ValidationMessage {
                                value: Vec::new(),
                                id: 0,
                                round: 0,
                            }));
                            // Duplicate the message to the new channel
                            let _ =
                                new_senders[index].send(OutMessage::Validate(validation_message));
                        }
                        // All other messages are for the network, so send these to new channels
                        message => {
                            let _ = new_senders[index].send(message);
                        }
                    };
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

/// This function takes the senders and receivers and will duplicate messages from all instances
/// and send those messages to all other instances.
///
/// This simulates a kind of broadcast network. Any instance that sends a message out will then be
/// sent to all others. This is done just for the network-bound OutMessage's.
fn emulate_gossip_network(
    mut _receivers: Vec<UnboundedReceiver<OutMessage>>,
    _senders: Vec<UnboundedSender<InMessage>>,
) {
} //-> Vec<UnboundedReceiver<OutMessage>> { }

#[tokio::test]
async fn test_initialization() {}
