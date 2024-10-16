use config::Config;
use std::cmp::Eq;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, warn};

pub use types::{
    CloseMessage, Completed, ConsensusData, InMessage, InstanceHeight, InstanceState,
    LeaderFunction, OperatorId, OutMessage, Round,
};

mod config;
mod error;
mod types;

#[cfg(test)]
mod tests;

/// The structure that defines the Quorum Based Fault Tolerance (QBFT) instance.
///
/// This builds and runs an entire QBFT process until it completes. It can complete either
/// successfully (i.e that it has successfully come to consensus, or through a timeout where enough
/// round changes have elapsed before coming to consensus.
pub struct Qbft<F, D>
where
    F: LeaderFunction + Clone,
    D: Debug + Clone + Eq + Hash,
{
    /// The initial configuration used to establish this instance of QBFT.
    config: Config<F>,
    /// The instance height acts as an ID for the current instance and helps distinguish it from
    /// other instances.
    instance_height: InstanceHeight,
    /// The current round this instance state is in.
    current_round: Round,
    /// If we have come to consensus in a previous round this is set here.
    previous_consensus: Option<ConsensusData<D>>,
    /// The messages received this round that we have collected to reach quorum.
    prepare_messages: HashMap<Round, HashMap<OperatorId, D>>,
    commit_messages: HashMap<Round, HashMap<OperatorId, D>>,
    /// Stores the round change messages. The second hashmap stores optional past consensus
    /// data for each round change message.
    round_change_messages: HashMap<Round, HashMap<OperatorId, Option<ConsensusData<D>>>>,
    // Channel that links the QBFT instance to the client processor and is where messages are sent
    // to be distributed to the committee
    message_out: UnboundedSender<OutMessage<D>>,
    // Channel that receives messages from the client processor
    message_in: UnboundedReceiver<InMessage<D>>,
    /// The current state of the instance
    state: InstanceState,
}

impl<F, D> Qbft<F, D>
where
    F: LeaderFunction + Clone,
    D: Debug + Clone + Hash + Eq,
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
            previous_consensus: None,
            prepare_messages: HashMap::with_capacity(estimated_map_size),
            commit_messages: HashMap::with_capacity(estimated_map_size),
            round_change_messages: HashMap::with_capacity(estimated_map_size),
            message_out,
            message_in,
            state: InstanceState::AwaitingProposal,
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
                            Some(InMessage::RecvData(consensus_data)) => self.received_data(consensus_data.round, consensus_data.data),
                            //When a Propose message is received, run the
                            // received_propose function
                            Some(InMessage::Propose(operator_id, consensus_data)) => self.received_propose(operator_id, consensus_data),
                            // When a Prepare message is received, run the
                            // received_prepare function
                            Some(InMessage::Prepare(operator_id, consensus_data)) => self.received_prepare(operator_id, consensus_data),
                            // When a Commit message is received, run the
                            // received_commit function
                            Some(InMessage::Commit(operator_id, consensus_data)) => self.received_commit(operator_id, consensus_data),
                            // When a RoundChange message is received, run the received_roundChange function
                            Some(InMessage::RoundChange(operator_id, round, maybe_past_consensus_data)) => self.received_round_change(operator_id, round, maybe_past_consensus_data),
                            // When a CloseRequest is received, close the instance
                            Some(InMessage::RequestClose(_close_message)) => {
                                // stub function in case we want to do anything pre-close
                                self.received_request_close();
                                break;
                            }

                        None => { }// Channel is closed
                    }

                }
                _ = round_end.tick() => {

                    // TODO: Leaving implement
                    debug!("ID{}: Round {} failed, incrementing round", *self.config.operator_id, *self.current_round);
                       if *self.current_round > self.config.max_rounds() {
                            self.send_completed(Completed::TimedOut);
                            // May not need break if can reliably close from client but keeping for now in case of bugs
                            break;
                       }
                       self.send_round_change();
                }
            }
        }
        debug!("ID{}: Instance killed", *self.config.operator_id);
    }

    fn operator_id(&self) -> OperatorId {
        self.config.operator_id
    }

    fn committee_members(&self) -> Vec<usize> {
        self.config.committee_members.clone()
    }

    fn get_f(&self) -> usize {
        let f = (self.config.committee_size - 1) % 3;
        if f > 0 {
            f
        } else {
            1
        }
    }

    fn send_message(&mut self, message: OutMessage<D>) {
        let _ = self.message_out.send(message);
    }

    fn set_previous_consensus(&mut self, round: Round, data: D) {
        debug!(
            "ID{}: Setting pr: {:?} and pv: {:?}",
            *self.operator_id(),
            *round,
            data
        );
        self.previous_consensus = Some(ConsensusData { round, data });
    }

    fn increment_round(&mut self) {
        self.current_round.increment();
        self.start_round();
    }

    // Validation and check functions
    fn check_leader(&self, operator_id: &OperatorId) -> bool {
        self.config.leader_fn.leader_function(
            operator_id,
            self.current_round,
            self.instance_height,
            self.config.committee_size,
        )
    }
    fn validate_data(&self, _data: &D) -> bool {
        true
    }
    fn check_committee(&self, operator_id: &usize) -> bool {
        self.committee_members().contains(operator_id)
    }

    // TODO: Store votes to avoid building this calc
    // TODO: Understand this quorum
    fn check_round_change_quorum(&mut self) {}
    /*
        if let Some(new_round_messages) = self.round_change_messages.get(&self.current_round) {
            let counter = new_round_messages.values().fold(
                HashMap::<usize, usize>::new(),
                |mut counter, message| {
                    *counter.entry(message.round_new).or_default() += 1;
                    counter
                },
            );
            if let Some((_, count)) = counter.into_iter().max_by_key(|&(_, v)| v) {
                if count >= self.config.quorum_size {
                    // Find the maximum `pr` value
                    if let Some((operator_id, max_value)) = new_round_messages
                        .iter()
                        .max_by_key(|(_, round_change)| round_change.pr)
                    {
                        if max_value.pr >= 1 && self.validate_data(max_value.pv.clone()).is_some() {
                            debug!(
                                "ID{}: The previouly stored maximum pr value is {} from operator_id {} with pv value {:?}",
                                self.operator_id(), max_value.pr, operator_id, max_value.pv
                            );
                            self.set_pr_pv(max_value.pr, max_value.pv.clone());
                        } else {
                            debug!("ID{}: Prepare consensus not reached previously and/or consensus data is none" , self.operator_id());
                        }
                    } else {
                        warn!("RoundChange quorum was not met");
                        //TODO: exit here with error
                    }
                }
            }
        }
    }
    */

    // Handles the beginning of a round.
    fn start_round(&mut self) {
        debug!(
            "ID{}: Round {} starting",
            *self.operator_id(),
            *self.current_round
        );

        if self.check_leader(&self.operator_id()) {
            // We are the leader
            debug!("ID{}: believes they are the leader", *self.operator_id());
            if matches!(self.state, InstanceState::SentRoundChange) {
                self.check_round_change_quorum();
            }

            self.state = InstanceState::AwaitingProposal;

            // If we have reached a previous consensus
            if let Some(previous_consensus) = self.previous_consensus.clone() {
                debug!(
                    "ID{}: pr: {} and pv: {:?} are set from previous round",
                    *self.operator_id(),
                    *previous_consensus.round,
                    previous_consensus.data
                );
                self.send_proposal(previous_consensus.data.clone());
                self.send_prepare(previous_consensus.data);
            } else {
                debug! {"ID{}: requesting data from client processor", *self.operator_id()}
                self.send_message(OutMessage::GetData(self.current_round));
            }
        } else {
            // We are not the leader, so await a proposal from a leader
            self.state = InstanceState::AwaitingProposal;
        }
    }

    /// Received message functions
    /// Received data to be sent as proposal
    fn received_data(&mut self, round: Round, data: D) {
        // Check that we are the leader to make sure this is a timely response, for whilst we are
        // still the leader and that we're awaiting a proposal
        if !(self.check_leader(&self.operator_id())
        // Verify we are in right committee
            && self.check_committee(&self.operator_id())
        // Verify that we are awaiting a proposal
            && matches!(self.state, InstanceState::AwaitingProposal)
            // Verify that his data is designed for this round
            && self.current_round == round)
        {
            warn!("We have received out of date consensus data: {:?}", data);
            return;
        }

        // If the data is valid, send a PROPOSAL and PREPARE
        if self.validate_data(&data) {
            debug!("ID{}: received data {:?}", *self.operator_id(), data);
            self.send_proposal(data.clone());
            self.send_prepare(data.clone());
        } else {
            error!("ID{}: Received invalid data", *self.operator_id());
        }
    }
    /// We have received a proposal message
    fn received_propose(&mut self, operator_id: OperatorId, consensus_data: ConsensusData<D>) {
        // Check if proposal is from the leader we expect
        if !(self.check_leader(&operator_id)
        // Check that this operator is in our committee
            && self.check_committee(&operator_id)
        // Check that we are awaiting a proposal
            && matches!(self.state, InstanceState::AwaitingProposal)
        //  Ensure that this message is for the correct round
        && self.current_round == consensus_data.round)
        {
            warn!("We have received an invalid propose message from: {:?}, data: {:?}, current_round: {}", operator_id, consensus_data, *self.current_round);
            return;
        }

        debug!(
            "ID{}: Proposal is from round leader with ID {}",
            *self.operator_id(),
            *operator_id,
        );
        // Updates the current state of quorum or not
        self.check_round_change_quorum();

        // Check to make sure the proposed data matches any previous consensus we have reached
        if let Some(previous_consensus) = self.previous_consensus.clone() {
            debug!(
                "ID{}: previous_round: {} and prevous_value: {:?} are set from previous round",
                *self.operator_id(),
                *previous_consensus.round,
                previous_consensus.data,
            );

            // Check if the proposed data matches the previous data
            if consensus_data.data == previous_consensus.data {
                self.send_prepare(previous_consensus.data.clone());
            } else {
                warn!(
                    "ID{}: Received data does not agree with stored previous consensus data",
                    *self.operator_id()
                );
            }
        } else {
            // We have no previous consensus data, simply validate the data and send the prepare
            if self.validate_data(&consensus_data.data) {
                // If of valid type, set data locally then send prepare
                self.send_prepare(consensus_data.data);
            } else {
                warn!("ID{}: Received invalid data", *self.operator_id());
            }
        }
    }

    /// We have received a prepare message
    fn received_prepare(&mut self, operator_id: OperatorId, consensus_data: ConsensusData<D>) {
        // Validate the message
        // Check if the prepare message is from the committee and the data is valid
        if !(self.check_committee(&operator_id)
        // Ensure the data is valid
            && self.validate_data(&consensus_data.data)
        // Ensure the data is for the current round
            && self.current_round == consensus_data.round)
        {
            warn!(
                ?operator_id,
                ?consensus_data,
                "Received and invalid prepare message."
            );
            return;
        }

        // Store the prepare message
        if self
            .prepare_messages
            .entry(consensus_data.round)
            .or_default()
            .insert(operator_id, consensus_data.data)
            .is_some()
        {
            warn!(
                "ID {}: Operator {:?} sent duplicate prepare",
                *self.operator_id(),
                operator_id
            )
        };

        // If we have stored round messages
        if let Some(round_messages) = self.prepare_messages.get(&self.current_round) {
            // Check the quorum size
            if round_messages.len() >= self.config.quorum_size {
                let counter = round_messages.values().fold(
                    HashMap::<&D, usize>::new(),
                    |mut counter, data| {
                        *counter.entry(&data).or_default() += 1;
                        counter
                    },
                );
                if let Some((data, count)) = counter.into_iter().max_by_key(|&(_, v)| v) {
                    if count >= self.config.quorum_size
                        && matches!(self.state, InstanceState::Prepare)
                    {
                        self.send_commit(data.clone());
                        self.set_previous_consensus(self.current_round, data.clone());
                    }
                }
            }
        }
    }

    ///We have received a commit message
    fn received_commit(&mut self, operator_id: OperatorId, consensus_data: ConsensusData<D>) {
        // Validate the received data

        if !(self.check_committee(&operator_id)
        // Ensure the data is valid
            && self.validate_data(&consensus_data.data)
        // Ensure the message is for the correct round
            && self.current_round == consensus_data.round)
        {
            warn!(
                ?operator_id,
                current_round = *self.current_round,
                ?consensus_data,
                "Invalid commit message received"
            );
            return;
        }

        // Store the received commit message
        if self
            .commit_messages
            .entry(self.current_round)
            .or_default()
            .insert(operator_id, consensus_data.data.clone())
            .is_some()
        {
            warn!(
                "ID {}: Operator {} sent duplicate commit",
                *self.operator_id(),
                *operator_id
            );
        }

        // Check if we have reached quorum
        if let Some(round_messages) = self.prepare_messages.get(&self.current_round) {
            // Check the quorum size
            if round_messages.len() >= self.config.quorum_size {
                let counter = round_messages.values().fold(
                    HashMap::<&D, usize>::new(),
                    |mut counter, value| {
                        *counter.entry(&value).or_default() += 1;
                        counter
                    },
                );
                if let Some((data, count)) = counter.into_iter().max_by_key(|&(_, v)| v) {
                    if count >= self.config.quorum_size
                        && matches!(self.state, InstanceState::Commit)
                    {
                        self.send_completed(Completed::Success(data.clone()));
                    }
                }
            }
        }
    }

    /// We have received a round change message.
    fn received_round_change(
        &mut self,
        operator_id: OperatorId,
        round: Round,
        maybe_past_consensus_data: Option<ConsensusData<D>>,
    ) {
        // Validate the round change message

        if !(self.check_committee(&operator_id)
        // The new round is larger than the current round
            && *round >= *self.current_round)
        {
            warn!(
                ?operator_id,
                current_round = *self.current_round,
                ?round,
                ?maybe_past_consensus_data,
                "Received an invalid round change message"
            );
            return;
        }

        // Store the round change message, for the round the message references
        if self
            .round_change_messages
            .entry(round)
            .or_default()
            .insert(operator_id, maybe_past_consensus_data.clone())
            .is_some()
        {
            warn!(
                "ID {}: Operator {} sent duplicate RoundChange request",
                *self.operator_id(),
                *operator_id
            );
        }

        // If we have past round change messages for this new round check the quorum size.
        // If we have reached quorum, then we transition to the next round.
        // TODO: Age - I'm not sure what the logic is here, check
        /*
        if let Some(new_round_messages) = self
            .round_change_messages
            .get(&round)
        {
            // Check the quorum size
            if new_round_messages.len() >= self.config.quorum_size
                && matches!(self.config.state, InstanceState::SentRoundChange)
            {
                let counter = new_round_messages.values().fold(
                    HashMap::<usize, usize>::new(),
                    |mut counter, message| {
                        *counter.entry(message.round_new).or_default() += 1;
                        counter
                    },
                );
                if let Some((new_round, count)) = counter.into_iter().max_by_key(|&(_, v)| v) {
                    if count >= self.config.quorum_size && new_round > self.current_round {
                        //debug!("ID{}: Quorum for round change request", self.operator_id());
                        self.increment_round(new_round);
                    }
                }
            } else if new_round_messages.len() >= self.get_f()
                && !(matches!(self.config.state, InstanceState::SentRoundChange))
            {
                let counter = new_round_messages.values().fold(
                    HashMap::<usize, usize>::new(),
                    |mut counter, message| {
                        *counter.entry(message.round_new).or_default() += 1;
                        counter
                    },
                );
                if let Some((new_round, count)) = counter.into_iter().max_by_key(|&(_, v)| v) {
                    if count >= self.config.quorum_size && new_round > self.current_round {
                        //debug!("ID{}: Quorum for round change request", self.operator_id());
                        self.increment_round(new_round);
                    }
                }
            }
        }
        */
    }

    fn received_request_close(&self) {
        debug!(
            "ID{}: State - {:?} -- Received close request",
            *self.operator_id(),
            self.state
        );
    }

    // Send message functions
    fn send_proposal(&mut self, data: D) {
        self.send_message(OutMessage::Propose(ConsensusData {
            round: self.current_round,
            data,
        }));
        self.state = InstanceState::Propose;
        debug!("ID{}: State - {:?}", *self.operator_id(), self.state);
    }

    fn send_prepare(&mut self, data: D) {
        let consensus_data = ConsensusData {
            round: self.current_round,
            data,
        };
        self.send_message(OutMessage::Prepare(consensus_data.clone()));
        //And store a prepare locally
        let operator_id = self.operator_id();
        self.prepare_messages
            .entry(self.current_round)
            .or_default()
            .insert(operator_id, consensus_data.data);

        self.state = InstanceState::Prepare;
        debug!("ID{}: State - {:?}", *self.operator_id(), self.state);
    }

    fn send_commit(&mut self, data: D) {
        let consensus_data = ConsensusData {
            round: self.current_round,
            data,
        };
        self.send_message(OutMessage::Commit(consensus_data.clone())); //And store a commit locally
        let operator_id = self.operator_id();
        self.commit_messages
            .entry(self.current_round)
            .or_default()
            .insert(operator_id, consensus_data.data);
        self.state = InstanceState::Commit;
        debug!("ID{}: State - {:?}", *self.operator_id(), self.state);
    }

    fn send_round_change(&mut self) {
        self.send_message(OutMessage::RoundChange(RoundChange {
            operator_id: self.operator_id(),
            instance_height: self.instance_height,
            round_new: self.current_round + 1,
            pr: self.config.pr,
            pv: self.config.pv.clone(),
        }));
        //And store locally
        self.store_messages(InMessage::RoundChange(RoundChange {
            operator_id: self.operator_id(),
            instance_height: self.instance_height,
            round_new: self.current_round + 1,
            pr: self.config.pr,
            pv: self.config.pv.clone(),
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
