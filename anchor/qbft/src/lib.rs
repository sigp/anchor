use config::{Config, LeaderFunction};
use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, warn};

mod config;
mod error;

#[cfg(test)]
mod tests;

type ValidationId = usize;
type Round = usize;
/// The structure that defines the Quorum Based Fault Tolerance (Qbft) instance
pub struct Qbft<F, D>
where
    F: LeaderFunction + Clone,
    D: Debug + Default + Clone,
{
    config: Config<F>,
    instance_height: usize,
    current_round: usize,
    data: D,
    /// ID used for tracking validation of messages
    current_validation_id: usize,
    /// Hashmap of validations that have been sent to the processor
    inflight_validations: HashMap<ValidationId, ValidationMessage<D>>, // TODO: Potentially unbounded
    /// The messages received this round that we have collected to reach quorum
    prepare_messages: HashMap<Round, Vec<PrepareMessage<D>>>,
    commit_messages: HashMap<Round, Vec<CommitMessage<D>>>,
    round_change_messages: HashMap<Round, Vec<RoundChange<D>>>,
    // some change
    /// commit_messages: HashMap<Round, Vec<PrepareMessage>>,
    // Channel that links the Qbft instance to the client processor and is where messages are sent
    // to be distributed to the committee
    message_out: UnboundedSender<OutMessage<D>>,
    // Channel that receives messages from the client processor
    message_in: UnboundedReceiver<InMessage<D>>,
}

/// Generic Data trait to allow for future implementations of the QBFT module
// Messages that can be received from the message_in channel
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum InMessage<D: Debug + Default + Clone> {
    /// A request for data to form consensus on if we are the leader.
    RecvData(GetData<D>),
    /// A PROPOSE message to be sent on the network.
    Propose(ProposeMessage<D>),
    /// A PREPARE message to be sent on the network.
    Prepare(PrepareMessage<D>),
    /// A commit message to be sent on the network.
    Commit(CommitMessage<D>),
    /// A validation request from the application to check if the message should be commited.
    Validate(ValidationMessage<D>),
    /// Round change message received from network
    RoundChange(RoundChange<D>),
}

/// Messages that may be sent to the message_out channel from the instance to the client processor
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum OutMessage<D: Debug + Default + Clone> {
    /// A request for data to form consensus on if we are the leader.
    GetData(GetData<D>),
    /// A PROPOSE message to be sent on the network.
    Propose(ProposeMessage<D>),
    /// A PREPARE message to be sent on the network.
    Prepare(PrepareMessage<D>),
    /// A commit message to be sent on the network.
    Commit(CommitMessage<D>),
    /// A validation request from the application to check if the message should be commited.
    Validate(ValidationMessage<D>),
    /// The round has ended, send this message to the network to inform all participants.
    RoundChange(RoundChange<D>),
    /// The consensus instance has completed.
    Completed(Completed<D>),
}
/// Type definitions for the allowable messages
#[allow(dead_code)]
#[derive(Debug, Clone)]

pub struct RoundChange<D: Debug + Default + Clone> {
    instance_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct GetData<D: Debug + Default + Clone> {
    instance_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[derive(Debug, Clone)]
pub struct ProposeMessage<D: Debug + Default + Clone> {
    instance_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PrepareMessage<D: Debug + Default + Clone> {
    instance_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CommitMessage<D: Debug + Default + Clone> {
    instance_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ValidationMessage<D: Debug + Default + Clone> {
    instance_id: ValidationId,
    instance_height: usize,
    round: usize,
    value: D,
}

/// Define potential outcome of validation of received proposal
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum ValidationOutcome {
    Success,
    Failure(ValidationError),
}

/// These are potential errors that may be returned from validation request -- likely only required for GetData operation for round leader
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// This means that lighthouse couldn't find the value
    LighthouseSaysNo,
    /// It doesn't exist and its wrong
    DidNotExist,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
/// The consensus instance has finished.
pub enum Completed<D> {
    /// The instance has timed out.
    TimedOut,
    /// Consensus was reached on the provided data.
    Success(D),
}

// TODO: Make a builder and validate config
// TODO: getters and setters for the config fields
// TODO: Remove this allow

#[allow(dead_code)]
impl<F, D> Qbft<F, D>
where
    F: LeaderFunction + Clone,
    D: Debug + Default + Clone,
{
    pub fn new(
        config: Config<F>,
    ) -> (
        UnboundedSender<InMessage<D>>,
        UnboundedReceiver<OutMessage<D>>,
        Self,
    ) {
        let (in_sender, message_in) = tokio::sync::mpsc::unbounded_channel();
        let (message_out, out_receiver) = tokio::sync::mpsc::unbounded_channel();

        let estimated_map_size = config.committee_size;
        let instance = Qbft {
            current_round: config.round,
            instance_height: config.instance_height,
            config,
            current_validation_id: 0,
            data: D::default(),
            inflight_validations: HashMap::with_capacity(100),
            prepare_messages: HashMap::with_capacity(estimated_map_size),
            commit_messages: HashMap::with_capacity(estimated_map_size),
            round_change_messages: HashMap::with_capacity(estimated_map_size),
            message_out,
            message_in,
        };

        (in_sender, out_receiver, instance)
    }

    pub async fn start_instance(mut self) {
        let mut round_end = tokio::time::interval(self.config.round_time);
        self.start_round();
        loop {
            tokio::select! {
                    message = self.message_in.recv() => {
                        match message {
                            // When a receive data message is received, run the
                            // received_data function
                            Some(InMessage::RecvData(received_data)) => self.received_data(received_data),
                            //When a Propose message is received, run the
                            // received_propose function
                            Some(InMessage::Propose(propose_message)) => self.received_propose(propose_message),
                            // When a Prepare message is received, run the
                            // received_prepare function
                            Some(InMessage::Prepare(received_prepare)) => self.received_prepare(received_prepare),
                            // When a Commit message is received, run the
                            // received_commit function
                            Some(InMessage::Commit(commit_message)) => self.received_commit(commit_message),
                            // When a RoundChange message is received, run the
                            // received_roundChange function
                            Some(InMessage::RoundChange(round_change_message)) => self.received_round_change(round_change_message),

                        // None => { }// Channel is closed
                        _ => {}
                                    // TODO: FILL THESE IN
                    }
                }
                _ = round_end.tick() => {

                    // TODO: Leaving implement
                    debug!("ID{}: Round {} failed, incrementing round", self.config.instance_id, self.current_round);
                        self.increment_round();
                                if self.current_round > 2 {
                            break;
                    }
                }
            }
        }
        debug!("ID{}: Instance killed", self.config.instance_id);
    }

    fn set_data(&mut self, data: D) {
        self.data = data;
    }

    fn instance_id(&self) -> usize {
        self.config.instance_id
    }

    //fn validate_data<>() -> bool {}
    // Check if the type is the specific type `D`

    fn send_message(&mut self, message: OutMessage<D>) {
        let _ = self.message_out.send(message);
    }

    fn increment_round(&mut self) {
        self.current_round += 1;
        self.start_round();
    }

    fn start_round(&mut self) {
        debug!(
            "ID{}: Round {} starting",
            self.instance_id(),
            self.current_round
        );

        if self.config.leader_fn.leader_function(
            self.instance_id(),
            self.current_round,
            self.instance_height,
            self.config.committee_size,
        ) {
            debug!("ID{}: believes they are the leader", self.instance_id());

            // TODO: Need to get data, then on recv do a proposal.
            // In the recv, we probably want to re-check if we are the leader before sending
            // propose, in case external app is slow
            self.send_message(OutMessage::GetData(GetData {
                instance_id: self.instance_id(),
                instance_height: self.instance_height,
                round: self.current_round,
                value: self.data.clone(),
            }));
        };
    }

    fn received_data(&mut self, message: GetData<D>) {
        if self.config.leader_fn.leader_function(
            self.instance_id(),
            self.current_round,
            self.instance_height,
            self.config.committee_size,
        ) && self.instance_height == message.instance_height
        {
            let data = message.value.clone();
            self.set_data(data);
            self.send_proposal()
        };
    }

    fn send_proposal(&mut self) {
        self.send_message(OutMessage::Propose(ProposeMessage {
            instance_id: self.instance_id(),
            instance_height: self.instance_height,
            round: self.current_round,
            value: self.data.clone(),
        }));

        // Also sends prepare
        let _ = self.message_out.send(OutMessage::Prepare(PrepareMessage {
            instance_id: self.instance_id(),
            instance_height: self.instance_height,
            round: self.current_round,
            value: self.data.clone(),
        }));

        // And store a prepare locally
        let instance_id = self.instance_id();
        self.prepare_messages
            .entry(self.current_round)
            .or_default()
            .push(PrepareMessage {
                instance_id,
                instance_height: self.instance_height,
                round: self.current_round,
                value: self.data.clone(),
            });
    }

    /// We have received a proposal from someone. We need to:
    ///
    /// 1. Check the proposer is a valid and who we expect
    /// 2. Check that the proposal is valid and we agree on the value --- have removed for now,
    fn received_propose(&mut self, propose_message: ProposeMessage<D>) {
        // Handle step 1.

        if self.config.leader_fn.leader_function(
            propose_message.instance_id,
            self.current_round,
            self.instance_height,
            self.config.committee_size,
        ) {
            let instance_id = self.instance_id();
            debug!(
                "ID {}: Proposal is from round leader with ID {}",
                instance_id, propose_message.instance_id,
            );

            // Validate the proposal with a local function that is is passed in frm the config
            // similar to the leaderfunction for now return bool -> true

            // Send propose message
            let _ = self.message_out.send(OutMessage::Prepare(PrepareMessage {
                instance_id,
                instance_height: self.instance_height,
                round: self.current_round,

                value: propose_message.value,
            }));

            // VALIDATE
            // let _ = self
            //     .message_out
            //     .send(OutMessage::Validate(ValidationMessage {
            //         id: self.current_validation_id,
            //         value: propose_message.value,
            //         round: self.current_round,
            //     }));
            //  self.current_validation_id += 1;

            return;
        }
    }

    /// The response to a validation request.
    ///
    /// If the proposal fails we drop the message, if it is successful, we send a prepare.
    //    fn validate_proposal(&mut self, validation_id: ValidationId, _outcome: ValidationOutcome) {
    //        let Some(validation_message) = self.inflight_validations.remove(&validation_id) else {
    //            warn!(validation_id, "Validation response without a request");
    //           return;
    //        };
    //
    //        let prepare_message = PrepareMessage {
    //            value: validation_message.value,
    //        };

    // If this errors its because the channel is closed and it's likely the application is
    // shutting down.
    // TODO: Come back to handle this, maybe end the instance
    //       let _ = self
    //           .message_out
    //           .send(OutMessage::Prepare(prepare_message.clone()));
    //    }

    /// We have received a PREPARE message
    ///
    /// If we have reached quorum then send a commit
    /// Otherwise store the prepare and wait for quorum.
    fn received_prepare(&mut self, prepare_message: PrepareMessage) {
        // TODO: Validate via sig generically for the committee.
        // So if in committee then:

        // Store the received prepare message
        // TODO: check each prepare is unique
        self.prepare_messages
            .entry(self.current_round)
            .or_default()
            .push(prepare_message);

        // Check Quorum
        // This is based on number of nodes in the group.
        // We have to make sure the value on all the prepares match
        if let Some(messages) = self.prepare_messages.get(&self.current_round) {
            // SEND commit
            if messages.len() >= self.config.quorum_size {
                let _ = self.message_out.send(OutMessage::Commit(CommitMessage {
                    value: vec![
                        self.instance_id(),
                        self.config.instance_height,
                        self.current_round,
                        1,
                    ],
                }));
            }
        }
    }
    fn received_commit(&mut self, commit_message: CommitMessage) {
        // Store the received commit message
        self.commit_messages
            .entry(self.current_round)
            .or_default()
            .push(commit_message);
    }

    fn received_round_change(&mut self, round_change_message: RoundChange<D>) {
        // Store the received commit message
        self.round_change_messages
            .entry(self.current_round)
            .or_default()
            .push(round_change_message);
    }
}
