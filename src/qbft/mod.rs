use config::{Config, LeaderFunction};
use std::collections::HashMap;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, warn};

mod config;
mod error;

#[cfg(test)]
mod tests;

// TODO: Build config.rs
// mod config;
// use config::{Config, ConfigBuilder};

type ValidationId = usize;
type Round = usize;
/// The structure that defines the Quorum Based Fault Tolerance (Qbft) instance
pub struct Qbft {
    config: Config,
    instance_id: usize,
    instance_height: usize,
    current_round: usize,
    committee_size: usize,

    /// ID used for tracking validation of messages
    current_validation_id: usize,
    /// Hashmap of validations that have been sent to the processor
    inflight_validations: HashMap<ValidationId, ValidationMessage>, // TODO: Potentially unbounded
    /// The messages received this round that we have collected to reach quorum
    prepare_messages: HashMap<Round, Vec<PrepareMessage>>,
    commit_messages: HashMap<Round, Vec<CommitMessage>>,
    round_change_messages: HashMap<Round, Vec<RoundChange>>,

    /// commit_messages: HashMap<Round, Vec<PrepareMessage>>,
    // Channel that links the Qbft instance to the client processor and is where messages are sent
    // to be distributed to the committee
    message_out: UnboundedSender<OutMessage>,
    // Channel that receives messages from the client processor
    message_in: UnboundedReceiver<InMessage>,
}

// Messages that can be received from the message_in channel
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum InMessage {
    /// A request for data to form consensus on if we are the leader.
    RecvData(GetData),
    /// A PROPOSE message to be sent on the network.
    Propose(ProposeMessage),
    /// A PREPARE message to be sent on the network.
    Prepare(PrepareMessage),
    /// A commit message to be sent on the network.
    Commit(CommitMessage),
    /// A validation request from the application to check if the message should be commited.
    Validate(ValidationMessage),
    /// Round change message received from network
    RoundChange(RoundChange),
}

/// Messages that may be sent to the message_out channel from the instance to the client processor
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum OutMessage {
    /// A request for data to form consensus on if we are the leader.
    GetData(GetData),
    /// A PROPOSE message to be sent on the network.
    Propose(ProposeMessage),
    /// A PREPARE message to be sent on the network.
    Prepare(PrepareMessage),
    /// A commit message to be sent on the network.
    Commit(CommitMessage),
    /// A validation request from the application to check if the message should be commited.
    Validate(ValidationMessage),
    /// The round has ended, send this message to the network to inform all participants.
    RoundChange(RoundChange),
    /// The consensus instance has completed.
    Completed(Completed),
}
/// Type definitions for the allowable messages
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RoundChange {
    value: Vec<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct GetData {
    value: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct ProposeMessage {
    value: Vec<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PrepareMessage {
    value: Vec<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CommitMessage {
    value: Vec<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ValidationMessage {
    id: ValidationId,
    value: Vec<usize>,
    round: usize,
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
pub enum Completed {
    /// The instance has timed out.
    TimedOut,
    /// Consensus was reached on the provided data.
    Success(Vec<usize>),
}

// TODO: Make a builder and validate config
// TODO: getters and setters for the config fields
// TODO: Remove this allow
#[allow(dead_code)]
impl Qbft {
    // TODO: Doc comment
    pub fn new(
        config: Config,
    ) -> (
        UnboundedSender<InMessage>,
        UnboundedReceiver<OutMessage>,
        Self,
    ) {
        let (in_sender, message_in) = tokio::sync::mpsc::unbounded_channel();
        let (message_out, out_receiver) = tokio::sync::mpsc::unbounded_channel();

        let estimated_map_size = config.committee_size;
        let instance = Qbft {
            current_round: config.round,
            instance_height: config.instance_height,
            instance_id: config.instance_id,
            committee_size: config.committee_size,
            config,
            current_validation_id: 0,
            inflight_validations: HashMap::with_capacity(100),
            prepare_messages: HashMap::with_capacity(estimated_map_size),
            commit_messages: HashMap::with_capacity(estimated_map_size),
            round_change_messages: HashMap::with_capacity(estimated_map_size),
            message_out,
            message_in,
        };

        debug!("{}", instance.instance_id);

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
                        // When a Propose message is received, run the
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
                    debug!("ID{}: Round {} failed, incrementing round", self.instance_id, self.current_round);
                        self.increment_round(self.current_round);
                               if self.current_round > 2 {
                            break;
                    }
                }
            }
        }
        debug!("ID{}: Instance killed", self.instance_id);
    }

    fn start_round(&mut self) {
        debug!(
            "ID{}: Round {} starting",
            self.instance_id, self.current_round
        );

        if self.config.leader_fn.leader_function(
            self.instance_id,
            self.current_round,
            self.instance_height,
            self.committee_size,
        ) {
            debug!("ID{}: believes they are the leader", self.instance_id);
            // Sends propopsal
            // TODO: Handle this error properly
            let _ = self.message_out.send(OutMessage::Propose(ProposeMessage {
                value: vec![
                    self.instance_id,
                    self.instance_height,
                    self.current_round,
                    1,
                ],
            }));
            // Also sends prepare
            let _ = self.message_out.send(OutMessage::Prepare(PrepareMessage {
                value: vec![
                    self.instance_id,
                    self.instance_height,
                    self.current_round,
                    1,
                ],
            }));
            // TODO: Store a prepare locally
            //self.prepare_messages
            // .entry(self.current_round)
            //  .or_default()
            //  .push(prepare_message);
        }
    }

    fn increment_round(&mut self, current_round: usize) -> usize {
        self.current_round = current_round + 1;
        self.start_round();
        current_round
    }

    fn received_data(&mut self, _data: GetData) {}

    /// We have received a proposal from someone. We need to:
    ///
    /// 1. Check the proposer is a valid and who we expect
    /// 2. Check that the proposal is valid and we agree on the value --- have removed for now,
    ///    commit this needs to happen?
    fn received_propose(&mut self, propose_message: ProposeMessage) {
        // Handle step 1.

        if self.config.leader_fn.leader_function(
            propose_message.value[0],
            self.current_round,
            self.instance_height,
            self.committee_size,
        ) {
            debug!(
                "ID {}: Proposal is from round leader with ID {}",
                self.instance_id, propose_message.value[0]
            );

            let _ = self.message_out.send(OutMessage::Prepare(PrepareMessage {
                value: vec![
                    self.instance_id,
                    self.instance_height,
                    self.current_round,
                    1,
                ],
            }));

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
                        self.instance_id,
                        self.instance_height,
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

    fn received_round_change(&mut self, round_change_message: RoundChange) {
        // Store the received commit message
        self.round_change_messages
            .entry(self.current_round)
            .or_default()
            .push(round_change_message);
    }
}
