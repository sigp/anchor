use config::{Config, LeaderFunction};
use std::collections::HashMap;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::warn;

mod config;
mod error;

// TODO: Build config.rs
// mod config;
// use config::{Config, ConfigBuilder};

type ValidationId = usize;
type Round = usize;
/// The structure that defines the Quorum Based Fault Tolerance (QBFT) instance
#[warn(clippy::upper_case_acronyms)]
pub struct QBFT {
    config: Config,

    current_round: usize,
    /// ID used for tracking validation of messages
    current_validation_id: usize,
    /// Hashmap of validations that have been sent to the processor
    inflight_validations: HashMap<ValidationId, ValidationMessage>, // TODO: Potentially unbounded
    /// The messages received this round that we have collected to reach quorum
    prepare_messages: HashMap<Round, Vec<PrepareMessage>>,
    confirm_messages: HashMap<Round, Vec<ConfirmMessage>>,
    round_change_messages: HashMap<Round, Vec<RoundChange>>,

    /// commit_messages: HashMap<Round, Vec<PrepareMessage>>,
    // Channel that links the QBFT instance to the client processor and is where messages are sent
    // to be distributed to the committee
    message_out: UnboundedSender<OutMessage>,
    // Channel that receives messages from the client processor
    message_in: UnboundedReceiver<InMessage>,
}

// Messages that can be received from the message_in channel
#[allow(dead_code)]
pub enum InMessage {
    /// A request for data to form consensus on if we are the leader.
    RecvData(GetData),
    /// A PROPOSE message to be sent on the network.
    Propose(ProposeMessage),
    /// A PREPARE message to be sent on the network.
    Prepare(PrepareMessage),
    /// A CONFIRM message to be sent on the network.
    Confirm(ConfirmMessage),
    /// A validation request from the application to check if the message should be confirmed.
    Validate(ValidationMessage),
    /// Round change message received from network
    RoundChange(RoundChange),
}

/// Messages that may be sent to the message_out channel from the instance to the client processor
#[allow(dead_code)]
pub enum OutMessage {
    /// A request for data to form consensus on if we are the leader.
    GetData(GetData),
    /// A PROPOSE message to be sent on the network.
    Propose(ProposeMessage),
    /// A PREPARE message to be sent on the network.
    Prepare(PrepareMessage),
    /// A CONFIRM message to be sent on the network.
    Confirm(ConfirmMessage),
    /// A validation request from the application to check if the message should be confirmed.
    Validate(ValidationMessage),
    /// The round has ended, send this message to the network to inform all participants.
    RoundChange(RoundChange),
}
/// Type definitions for the allowable messages
#[allow(dead_code)]
pub struct RoundChange {
    value: Vec<u8>,
}

#[allow(dead_code)]
pub struct GetData {
    value: Vec<u8>,
}

pub struct ProposeMessage {
    value: Vec<u8>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct PrepareMessage {
    value: Vec<u8>,
}

#[allow(dead_code)]
pub struct ConfirmMessage {
    value: Vec<u8>,
}

#[allow(dead_code)]
pub struct ValidationMessage {
    id: ValidationId,
    value: Vec<u8>,
    round: usize,
}

/// Define potential outcome of validation of received proposal
#[allow(dead_code)]
pub enum ValidationOutcome {
    Success,
    Failure(ValidationError),
}

/// These are potential errors that may be returned from validation request -- likely only required for GetData operation for round leader
#[allow(dead_code)]
pub enum ValidationError {
    /// This means that lighthouse couldn't find the value
    LighthouseSaysNo,
    /// It doesn't exist and its wrong
    DidNotExist,
}

// TODO: Make a builder and validate config
// TODO: getters and setters for the config fields
// TODO: Remove this allow
#[allow(dead_code)]
impl QBFT {
    // TODO: Doc comment
    pub fn new(
        config: Config,
        sender: UnboundedSender<OutMessage>,
    ) -> (UnboundedSender<InMessage>, Self) {
        let (in_sender, in_receiver) = tokio::sync::mpsc::unbounded_channel();

        let estimated_map_size = config.quorum_size;
        // Validate Quorum size, cannot be 0 -- to be handled in config builder
        let instance = QBFT {
            current_round: config.round,
            config,
            current_validation_id: 0,
            inflight_validations: HashMap::with_capacity(100),
            prepare_messages: HashMap::with_capacity(estimated_map_size),
            confirm_messages: HashMap::with_capacity(estimated_map_size),
            round_change_messages: HashMap::with_capacity(estimated_map_size),
            message_out: sender,
            message_in: in_receiver,
        };

        (in_sender, instance)
    }

    pub async fn start_instance(&mut self) {
        self.start_round();
        let mut round_end = tokio::time::interval(self.config.round_time);
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
                                   // When a Confirm message is received, run the
                                   // received_confirm function
                                                   Some(InMessage::Confirm(confirm_message)) => self.received_confirm(confirm_message),
                                   // When a RoundChange message is received, run the
                                   // received_roundChange function
            Some(InMessage::RoundChange(round_change_message)) => self.received_round_change(round_change_message),

                                                   // TODO: FILL THESE IN
                                                   // None => { }// Channel is closed
                                                   _ => {}
                                               }
                                           }
                                        _ = round_end.tick() => {
                                               /*
                                               if self.round >= self.max_round {
                                                   break;
                       */
                       }

                                          //     self.increment_round()

                                   }
        }
    }

    fn start_round(&mut self) {
        if self.config.leader_fn.leader_function(self.current_round) {
            // TODO: Handle this error properly
            let _ = self
                .message_out
                .send(OutMessage::Propose(ProposeMessage { value: vec![0] }));
        }
    }

    fn increment_round(&mut self) {
        self.start_round();
    }

    fn received_data(&mut self, _data: GetData) {}
    /// We have received a proposal from someone. We need to:
    ///
    /// 1. Check the proposer is a valid and who we expect
    /// 2. Check that the proposal is valid and we agree on the value
    fn received_propose(&mut self, propose_message: ProposeMessage) {
        // Handle step 1.
        if !self.config.leader_fn.leader_function(self.current_round) {
            return;
        }
        // Step 2
        // TODO: Handle this error properly
        let _ = self
            .message_out
            .send(OutMessage::Validate(ValidationMessage {
                id: self.current_validation_id,
                value: propose_message.value,
                round: self.current_round,
            }));
        self.current_validation_id += 1;
    }

    /// The response to a validation request.
    ///
    /// If the proposal fails we drop the message, if it is successful, we send a prepare.
    fn validate_proposal(&mut self, validation_id: ValidationId, _outcome: ValidationOutcome) {
        let Some(validation_message) = self.inflight_validations.remove(&validation_id) else {
            warn!(validation_id, "Validation response without a request");
            return;
        };

        let prepare_message = PrepareMessage {
            value: validation_message.value,
        };

        // If this errors its because the channel is closed and it's likely the application is
        // shutting down.
        // TODO: Come back to handle this, maybe end the instance
        let _ = self
            .message_out
            .send(OutMessage::Prepare(prepare_message.clone()));

        // TODO: Store a prepare locally
        self.prepare_messages
            .entry(self.current_round)
            .or_default()
            .push(prepare_message);
    }

    /// We have received a PREPARE message
    ///
    /// If we have reached quorum then send a CONFIRM
    /// Otherwise store the prepare and wait for quorum.
    fn received_prepare(&mut self, prepare_message: PrepareMessage) {
        // Some kind of validation, legit person? legit group, legit round?

        // Make sure the value matches

        // Store the received prepare message
        self.prepare_messages
            .entry(self.current_round)
            .or_default()
            .push(prepare_message);

        // Check Quorum
        // This is based on number of nodes in the group.
        // TODO: Prob need to be more robust here
        // We have to make sure the value on all the prepare's match
        if let Some(messages) = self.prepare_messages.get(&self.current_round) {
            if messages.len() >= self.config.quorum_size {
                // SEND CONFIRM
            }
        }
    }
    fn received_confirm(&mut self, confirm_message: ConfirmMessage) {
        // Store the received confirm message
        self.confirm_messages
            .entry(self.current_round)
            .or_default()
            .push(confirm_message);
    }

    fn received_round_change(&mut self, round_change_message: RoundChange) {
        // Store the received confirm message
        self.round_change_messages
            .entry(self.current_round)
            .or_default()
            .push(round_change_message);
    }
}
#[cfg(test)]
mod tests {
    // use super::*;
    // use futures::StreamExt;

    /*
    #[tokio::test]
    async fn test_initialization() {

        let config = Config {
            id: 0,
            quorum_size: 5,
            round: 0,
            round_time: Duration::from_secs(1),
            leader_fn: LeaderFunctionStub {}
        };

        let (sender,receiver) = tokio::mpsc::UnboundedChannel();

        let (qbft_sender, qbft) = QBFT::new(config, sender);

        tokio::task::spawn(qbft.start_instance().await);

        loop {
            match receiver.await {
                OutMessageValidate => qbft_sender.send(Validated);
            }
        }
    }
    */
}
