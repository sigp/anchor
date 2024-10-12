use config::{Config, LeaderFunction};
use std::cmp::Eq;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, warn};

mod config;
mod error;

#[cfg(test)]
mod tests;

type ValidationId = usize;
type Round = usize;
type OperatorId = usize;

/// The structure that defines the Quorum Based Fault Tolerance (Qbft) instance
pub struct Qbft<F, D>
where
    F: LeaderFunction + Clone,
    D: Debug + Clone + Eq + Hash,
{
    config: Config<F>,
    instance_height: usize,
    current_round: usize,
    data: Option<D>,
    /// The messages received this round that we have collected to reach quorum
    prepare_messages: HashMap<Round, HashMap<OperatorId, PrepareMessage<D>>>,
    commit_messages: HashMap<Round, HashMap<OperatorId, CommitMessage<D>>>,
    round_change_messages: HashMap<Round, HashMap<OperatorId, RoundChange<D>>>,
    // Channel that links the Qbft instance to the client processor and is where messages are sent
    // to be distributed to the committee
    message_out: UnboundedSender<OutMessage<D>>,
    // Channel that receives messages from the client processor
    message_in: UnboundedReceiver<InMessage<D>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum InstanceState {
    AwaitingProposal,
    Propose,
    Prepare,
    Commit,
    SentRoundChange,
    Complete,
}

/// Generic Data trait to allow for future implementations of the QBFT module
// Messages that can be received from the message_in channel
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum InMessage<D: Debug + Clone + Eq + Hash> {
    /// A request for data to form consensus on if we are the leader.
    RecvData(RecvData<D>),
    /// A PROPOSE message to be sent on the network.
    Propose(ProposeMessage<D>),
    /// A PREPARE message to be sent on the network.
    Prepare(PrepareMessage<D>),
    /// A commit message to be sent on the network.
    Commit(CommitMessage<D>),
    /// Round change message received from network
    RoundChange(RoundChange<D>),
}

/// Messages that may be sent to the message_out channel from the instance to the client processor
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum OutMessage<D: Debug + Clone + Eq + Hash> {
    /// A request for data to form consensus on if we are the leader.
    GetData(GetData),
    /// A PROPOSE message to be sent on the network.
    Propose(ProposeMessage<D>),
    /// A PREPARE message to be sent on the network.
    Prepare(PrepareMessage<D>),
    /// A commit message to be sent on the network.
    Commit(CommitMessage<D>),
    /// The round has ended, send this message to the network to inform all participants.
    RoundChange(RoundChange<D>),
    /// The consensus instance has completed.
    Completed(CompletedMessage<D>),
}
/// Type definitions for the allowable messages
#[allow(dead_code)]
#[derive(Debug, Clone)]

pub struct RoundChange<D: Debug + Clone + Eq + Hash> {
    operator_id: usize,
    instance_height: usize,
    round_new: usize,
    pr: usize,
    pv: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct GetData {
    operator_id: usize,
    instance_height: usize,
    round: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RecvData<D: Debug + Clone + Eq + Hash> {
    operator_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProposeMessage<D: Debug + Clone + Eq + Hash> {
    operator_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct PrepareMessage<D: Debug + Clone + Eq + Hash> {
    operator_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CommitMessage<D: Debug + Clone + Eq + Hash> {
    operator_id: usize,
    instance_height: usize,
    round: usize,
    value: D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CompletedMessage<D: Debug + Clone + Eq + Hash> {
    operator_id: ValidationId,
    instance_height: usize,
    round: usize,
    completion_status: Completed<D>,
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
    D: Debug + Clone + Eq + Hash + Hash + Eq,
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
            data: None,
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

                        None => { }// Channel is closed
                    }
                }
                _ = round_end.tick() => {

                    // TODO: Leaving implement
                    debug!("ID{}: Round {} failed, incrementing round", self.config.operator_id, self.current_round);
                        self.increment_round();
                                if self.current_round > 2 {
                            self.send_completed(Completed::TimedOut);
                            break;
                    }
                }
            }
        }
        debug!("ID{}: Instance killed", self.config.operator_id);
    }

    //Get and set functions
    fn set_state(&mut self, new_state: InstanceState) {
        self.config.state = new_state;
    }
    fn set_data(&mut self, data: D) {
        self.data = Some(data);
    }
    fn operator_id(&self) -> usize {
        self.config.operator_id
    }
    fn committee_members(&self) -> Vec<usize> {
        self.config.committee_members.clone()
    }
    fn send_message(&mut self, message: OutMessage<D>) {
        let _ = self.message_out.send(message);
    }
    fn set_pr(&mut self, round: Round) {
        self.config.pr = round;
    }
    fn increment_round(&mut self) {
        self.current_round += 1;
        self.start_round();
    }
    fn store_messages(&mut self, in_message: InMessage<D>) {
        match in_message {
            InMessage::RecvData(_message) => {
                warn! {"ID {}: called store message on RecvData", self.operator_id()}
            }

            InMessage::Propose(_message) => {
                warn! {"ID {}: called  store message on Propose", self.operator_id()}
            }

            InMessage::Prepare(message) => {
                if self
                    .prepare_messages
                    .entry(message.round)
                    .or_default()
                    .insert(message.operator_id, message.clone())
                    .is_some()
                {
                    warn!(
                        "ID {}: Operator {} sent duplicate prepare",
                        self.operator_id(),
                        message.operator_id
                    )
                };
            }

            InMessage::Commit(message) => {
                if self
                    .commit_messages
                    .entry(message.round)
                    .or_default()
                    .insert(message.operator_id, message.clone())
                    .is_some()
                {
                    warn!(
                        "ID {}: Operator {} sent duplicate commit",
                        self.operator_id(),
                        message.operator_id
                    );
                }
            }

            InMessage::RoundChange(message) => {
                if self
                    .round_change_messages
                    .entry(message.round_new)
                    .or_default()
                    .insert(message.operator_id, message.clone())
                    .is_some()
                {
                    warn!(
                        "ID {}: Operator {} sent duplicate RoundChange request",
                        self.operator_id(),
                        message.operator_id
                    );
                }
            }
        }
    }

    //Validation and check functions
    fn check_leader(&self, operator_id: usize) -> bool {
        self.config.leader_fn.leader_function(
            operator_id,
            self.current_round,
            self.instance_height,
            self.config.committee_size,
        )
    }
    fn validate_data(&self, data: D) -> Option<D> {
        Some(data)
    }
    fn check_committee(&self, operator_id: &usize) -> bool {
        self.committee_members().contains(operator_id)
    }

    //Round start function
    fn start_round(&mut self) {
        self.set_state(InstanceState::AwaitingProposal);
        debug!(
            "ID{}: Round {} starting",
            self.operator_id(),
            self.current_round
        );

        if self.check_leader(self.operator_id()) {
            debug!("ID{}: believes they are the leader", self.operator_id());

            self.send_message(OutMessage::GetData(GetData {
                operator_id: self.operator_id(),
                instance_height: self.instance_height,
                round: self.current_round,
            }));
        };
    }

    /// Received message functions
    /// Received data to be sent as proposal
    fn received_data(&mut self, message: RecvData<D>) {
        // Check that we are the leader to make sure this is a timely response, for whilst we are
        // still the leader and that we're awaiting a proposal
        if self.check_leader(self.operator_id())
            && self.check_committee(&self.operator_id())
            && matches!(self.config.state, InstanceState::AwaitingProposal)
        {
            if let Some(data) = self.validate_data(message.value.clone()) {
                debug!(
                    "ID{}: received data {:?}",
                    self.operator_id(),
                    message.value.clone()
                );
                self.set_data(data.clone());
                self.send_proposal(data.clone());
                self.send_prepare(data);
            } else {
                error!("ID{}: Received invalid data", self.operator_id());
            }
        }
    }
    /// We have received a proposal message
    fn received_propose(&mut self, propose_message: ProposeMessage<D>) {
        // Check if proposal is from the leader we expect
        if self.check_leader(propose_message.operator_id)
            && self.check_committee(&propose_message.operator_id)
            && matches!(self.config.state, InstanceState::AwaitingProposal)
        {
            let self_operator_id = self.operator_id();
            debug!(
                "ID {}: Proposal is from round leader with ID {}",
                self_operator_id, propose_message.operator_id,
            );
            // Validate the proposal with a local function that is is passed in from the config
            // similar to the leaderfunction for now return bool -> true
            if let Some(data) = self.validate_data(propose_message.value) {
                // If of valid type, set data locally then send prepare
                self.set_data(data.clone());
                self.send_prepare(data);
            }
        }
    }

    /// We have received a prepare message
    fn received_prepare(&mut self, prepare_message: PrepareMessage<D>) {
        // Check if the prepare message is from the committee and the data is valid
        if self.check_committee(&prepare_message.operator_id)
            && self.validate_data(prepare_message.value.clone()).is_some()
            && matches!(self.config.state, InstanceState::Prepare)
        {
            self.store_messages(InMessage::Prepare(prepare_message.clone()));
            // If we have stored round messages
            if let Some(round_messages) = self.prepare_messages.get(&prepare_message.round) {
                // Check the quorum size
                if round_messages.len() >= self.config.quorum_size {
                    let counter = round_messages.values().fold(
                        HashMap::<&D, usize>::new(),
                        |mut counter, message| {
                            *counter.entry(&message.value).or_default() += 1;
                            counter
                        },
                    );
                    if let Some((data, count)) = counter.into_iter().max_by_key(|&(_, v)| v) {
                        if count >= self.config.quorum_size {
                            self.send_commit(data.clone());
                        }
                    }
                }
            }
        }
    }

    ///We have received a commit message
    fn received_commit(&mut self, commit_message: CommitMessage<D>) {
        if self.check_committee(&commit_message.operator_id)
            && self.validate_data(commit_message.value.clone()).is_some()
            && matches!(self.config.state, InstanceState::Prepare)
        {
            // Store the received commit message
            self.store_messages(InMessage::Commit(commit_message.clone()));
            if let Some(round_messages) = self.prepare_messages.get(&commit_message.round) {
                // Check the quorum size
                if round_messages.len() >= self.config.quorum_size {
                    let counter = round_messages.values().fold(
                        HashMap::<&D, usize>::new(),
                        |mut counter, message| {
                            *counter.entry(&message.value).or_default() += 1;
                            counter
                        },
                    );
                    if let Some((data, count)) = counter.into_iter().max_by_key(|&(_, v)| v) {
                        if count >= self.config.quorum_size {
                            self.send_completed(Completed::Success(data.clone()));
                        }
                    }
                }
            }
        }
    }

    fn received_round_change(&mut self, round_change_message: RoundChange<D>) {
        // Store the received commit message
        if self
            .round_change_messages
            .entry(round_change_message.round_new)
            .or_default()
            .insert(
                round_change_message.operator_id,
                round_change_message.clone(),
            )
            .is_some()
        {
            warn!(
                operator = round_change_message.operator_id,
                "Operator sent round change request"
            );
        }
    }
    //Send message functions
    fn send_proposal(&mut self, data: D) {
        self.send_message(OutMessage::Propose(ProposeMessage {
            operator_id: self.operator_id(),
            instance_height: self.instance_height,
            round: self.current_round,
            value: data,
        }));
        self.set_state(InstanceState::Propose);
        debug!("ID{}: State - {:?}", self.operator_id(), self.config.state);
    }

    fn send_prepare(&mut self, data: D) {
        self.send_message(OutMessage::Prepare(PrepareMessage {
            operator_id: self.operator_id(),
            instance_height: self.instance_height,
            round: self.current_round,
            value: data.clone(),
        }));
        //And store a prepare locally
        let operator_id = self.operator_id();
        self.prepare_messages
            .entry(self.current_round)
            .or_default()
            .insert(
                operator_id,
                PrepareMessage {
                    operator_id,
                    instance_height: self.instance_height,
                    round: self.current_round,
                    value: data,
                },
            );
        self.set_state(InstanceState::Prepare);
        debug!("ID{}: State - {:?}", self.operator_id(), self.config.state);
    }

    fn send_commit(&mut self, data: D) {
        self.send_message(OutMessage::Commit(CommitMessage {
            operator_id: self.operator_id(),
            instance_height: self.instance_height,
            round: self.current_round,
            value: data.clone(),
        }));
        //And store a commit locally
        let operator_id = self.operator_id();
        self.commit_messages
            .entry(self.current_round)
            .or_default()
            .insert(
                operator_id,
                CommitMessage {
                    operator_id,
                    instance_height: self.instance_height,
                    round: self.current_round,
                    value: data,
                },
            );
        self.set_state(InstanceState::Commit);
        debug!("ID{}: State - {:?}", self.operator_id(), self.config.state);
    }

    fn send_round_change(&mut self, data: D) {
        self.send_message(OutMessage::RoundChange(RoundChange {
            operator_id: self.operator_id(),
            instance_height: self.instance_height,
            round_new: self.current_round + 1,
            pr: self.config.pr,
            pv: data,
        }));
        self.set_state(InstanceState::SentRoundChange);
        debug!("ID{}: State - {:?}", self.operator_id(), self.config.state);
    }
    fn send_completed(&mut self, completion_status: Completed<D>) {
        self.send_message(OutMessage::Completed(CompletedMessage {
            operator_id: self.operator_id(),
            instance_height: self.instance_height,
            round: self.current_round,
            completion_status,
        }));
        self.set_state(InstanceState::Complete);
        debug!("ID{}: State - {:?}", self.operator_id(), self.config.state);
    }
}
