use std::collections::HashMap;
use tracing::warn;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Duration;

// TODO: Build config.rs
// mod config;
// use config::{Config, ConfigBuilder};

type ValidationId = usize;
type Round = usize;
/// The structure that defines the QBFT thing
pub struct QBFT {
    id: usize,
    quorum_size: usize,
    /// This the round of the current QBFT
    round: Round,
    round_time: Duration,
    leader_fn: LeaderFunctionStubStruct,
    current_validation_id: usize,
    inflight_validations: HashMap<ValidationId, ValidationMessage>, // TODO: Potentially unbounded
    /// The current messages for this round that we collected to reach quorum.
    prepare_messages: HashMap<Round, Vec<PrepareMessage>>,
    /// commit_messages: HashMap<Round, Vec<PrepareMessage>>,
    message_out: UnboundedSender<OutMessage>,
    message_in: UnboundedReceiver<InMessage>,
}

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
    /// The round has ended, send this message to the network to inform all participants.
    RoundChange(RoundChange),
}

/// The message that is sent outside of the QBFT instance to be handled externally.
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

pub struct RoundChange {
    value: Vec<u8>,
}

pub struct GetData {
    value: Vec<u8>,
}

pub struct ProposeMessage {
    value: Vec<u8>,
}

#[derive(Clone)]
pub struct PrepareMessage {
    value: Vec<u8>,
}

pub struct ConfirmMessage {
    value: Vec<u8>,
}

pub struct ValidationMessage {
    id: ValidationId,
    value: Vec<u8>,
    round: usize,
}

pub enum ValidationOutcome {
    Success,
    Failure(ValidationError),
}

/// This is the kind of error that can happen on a validation
pub enum ValidationError {
    /// This means that lighthouse couldn't find the value
    LighthouseSaysNo,
    /// It doesn't exist and its wrong
    DidNotExist,
}

pub trait LeaderFunction {
    /// Returns true if we are the leader
    fn leader_function(&self) -> bool;
}

pub struct LeaderFunctionStubStruct {
    random_var: String,
}

impl LeaderFunction for LeaderFunctionStubStruct {
    fn leader_function(&self) -> bool {
        if self.random_var == String::from("4") {
            true
        } else {
            false
        }
    }
}

// TODO: Doc comment this
pub struct Config {
    pub(crate) id: usize,
    quorum_size: usize,
    round: usize,
    round_time: Duration,
    leader_fn: LeaderFunctionStubStruct,
}

// TODO: Make a builder and validate config

impl QBFT {
    // TODO: Doc comment
    pub fn new(
        config: Config,
        sender: UnboundedSender<OutMessage>,
    ) -> (UnboundedSender<InMessage>, Self) {
        let Config {
            id,
            quorum_size,
            round,
            round_time,
            leader_fn,
        } = config;

        let (in_sender, in_receiver) = tokio::sync::mpsc::unbounded_channel();

        // Validate Quorum size, cannot be 0
        let instance = QBFT {
            id,
            quorum_size,
            round,
            round_time,
            leader_fn,
            current_validation_id: 0,
            inflight_validations: HashMap::with_capacity(100),
            prepare_messages: HashMap::with_capacity(quorum_size),
            message_out: sender,
            message_in: in_receiver,
        };

        (in_sender, instance)
    }

    pub async fn start_instance(&mut self) {
        self.start_round();
        let mut round_end = tokio::time::interval(self.round_time);
        loop {
            tokio::select! {
                    message = self.message_in.recv() => {
                        match message {
                            Some(InMessage::RecvData(recieved_data)) => self.recieved_data(recieved_data),
                            Some(InMessage::Propose(propose_message)) => self.received_propose(propose_message),
                            // TODO: FILL THESE IN
                            // None => { }// Channel is closed
                            _ => {}
                        }
                    }
                 _ = round_end.tick() => self.increment_round()

            }
        }
    }

    fn start_round(&mut self) {
        if self.leader_fn.leader_function() {
            self.message_out
                .send(OutMessage::Propose(ProposeMessage { value: vec![0] }));
        }
    }

    fn increment_round(&mut self) {
        self.start_round();
    }

    fn recieved_data(&mut self, _data: GetData) {}
    /// We have received a proposal from someone. We need to:
    ///
    /// 1. Check the proposer is a valid and who we expect
    /// 2. Check that the proposal is valid and we agree on the value
    fn received_propose(&mut self, propose_message: ProposeMessage) {
        // Handle step 1.
        if !self.leader_fn.leader_function() {
            return;
        }
        // Step 2
        self.message_out
            .send(OutMessage::Validate(ValidationMessage {
                id: self.current_validation_id,
                value: propose_message.value,
                round: self.round,
            }));
        self.current_validation_id += 1;
    }

    /// The response to a validation request.
    ///
    /// If the proposal fails we drop the message, if it is successful, we send a prepare.
    fn validate_proposal(&mut self, validation_id: ValidationId, outcome: ValidationOutcome) {
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
            .entry(self.round)
            .or_default()
            .push(prepare_message);
    }

    /// We have received a PREPARE message
    ///
    /// If we have reached quorum then send a CONFIRM
    /// Otherwise store the prepare and wait for quorum.
    fn _received_prepare(&mut self, prepare_message: PrepareMessage) {
        // Some kind of validation, legit person? legit group, legit round?

        // Make sure the value matches

        // Store the prepare we received
        self.prepare_messages
            .entry(self.round)
            .or_default()
            .push(prepare_message);

        // Check Quorum
        // This is based on number of nodes in the group.
        // TODO: Prob need to be more robust here
        // We have to make sure the value on all the prepare's match
        if let Some(messages) = self.prepare_messages.get(&self.round) {
            if messages.len() >= self.quorum_size {
                // SEND CONFIRM
            }
        }
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
            match reciever.await {
                OutMessageValidate => qbft_sender.send(Validated);
            }
        }
    }
    */
}
