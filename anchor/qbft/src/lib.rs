pub use config::{Config, ConfigBuilder};
use std::cmp::Eq;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, instrument, warn, Level};
pub use validation::{validate_consensus_data, ValidatedData, ValidationError};

pub use types::{
    Completed, ConsensusData, InMessage, InstanceHeight, InstanceState, LeaderFunction, OperatorId,
    OutMessage, Round,
};

mod config;
mod error;
mod types;
mod validation;

#[cfg(test)]
mod tests;

type RoundChangeMap<D> = HashMap<OperatorId, Option<ConsensusData<ValidatedData<D>>>>;

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
    /// Initial data that we will propose if we are the leader.
    start_data: ValidatedData<D>,
    /// The instance height acts as an ID for the current instance and helps distinguish it from
    /// other instances.
    instance_height: InstanceHeight,
    /// The current round this instance state is in.a
    current_round: Round,
    /// If we have come to consensus in a previous round this is set here.
    past_consensus: HashMap<Round, ValidatedData<D>>,
    /// The messages received this round that we have collected to reach quorum.
    prepare_messages: HashMap<Round, HashMap<ValidatedData<D>, HashSet<OperatorId>>>,
    commit_messages: HashMap<Round, HashMap<ValidatedData<D>, HashSet<OperatorId>>>,
    /// Stores the round change messages. The second hashmap stores optional past consensus
    /// data for each round change message.
    round_change_messages: HashMap<Round, RoundChangeMap<D>>,
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
        start_data: ValidatedData<D>,
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
            start_data,
            past_consensus: HashMap::with_capacity(2),
            prepare_messages: HashMap::with_capacity(estimated_map_size),
            commit_messages: HashMap::with_capacity(estimated_map_size),
            round_change_messages: HashMap::with_capacity(estimated_map_size),
            message_out,
            message_in,
            state: InstanceState::AwaitingProposal,
        };

        (in_sender, out_receiver, instance)
    }

    // This adds the fields to all our logs for this instance.
    #[instrument(name = "QBFT",skip_all, fields(operator_id=*self.config.operator_id,instance_height=*self.config.instance_height), level= Level::ERROR)]
    pub async fn start_instance(mut self) {
        let mut round_end = tokio::time::interval(self.config.round_time);
        self.start_round();
        loop {
            // If we reached a critical error, end gracefully
            if matches!(self.state, InstanceState::Complete) {
                return;
            }

            tokio::select! {
                    message = self.message_in.recv() => {
                        match message {
                            // When a Propose message is received, run the
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
                            None => { } // Channel is closed
                    }

                }
                _ = round_end.tick() => {

                    debug!(round = *self.current_round,"Incrementing round");
                       if *self.current_round > self.config.max_rounds() {
                            self.send_completed(Completed::TimedOut);
                            break;
                       }
                       self.send_round_change(self.current_round.next());
                        // Start a new round
                       self.set_round(self.current_round.next());
                }
            }
        }
        debug!("Instance killed");
    }

    /// Returns the operator id for this instance.
    fn operator_id(&self) -> OperatorId {
        self.config.operator_id
    }

    /// Obtains the maximum number of faulty nodes that this consensus can tolerate
    fn get_f(&self) -> usize {
        let f = (self.config.committee_size - 1) % 3;
        if f > 0 {
            f
        } else {
            1
        }
    }

    /// Sends an outbound message
    fn send_message(&mut self, message: OutMessage<D>) {
        if self.message_out.send(message).is_err() {
            // The outbound channel has been closed. This instance can no longer progress. We
            // should terminate the current running instance
            warn!(
                instance_height = *self.config.instance_height,
                "Receiver channel closed. Terminating"
            );
            self.state = InstanceState::Complete
        }
    }

    /// Once we have achieved consensus on a PREPARE round, we add the data to mapping to match
    /// against later.
    fn insert_consensus(&mut self, round: Round, data: ValidatedData<D>) {
        debug!(round = *round, ?data, "Reached prepare consensus");
        if let Some(past_data) = self.past_consensus.insert(round, data.clone()) {
            warn!(round = *round, ?data, past_data = ?past_data, "Adding duplicate consensus data");
        }
    }

    /// Shifts this instance into a new round>
    fn set_round(&mut self, new_round: Round) {
        self.current_round.set(new_round);
        self.start_round();
    }

    // Validation and check functions.
    fn check_leader(&self, operator_id: &OperatorId) -> bool {
        self.config.leader_fn.leader_function(
            operator_id,
            self.current_round,
            self.instance_height,
            self.config.committee_size,
        )
    }

    /// Checks to make sure any given operator is in this instance's comittee.
    fn check_committee(&self, operator_id: &OperatorId) -> bool {
        self.config.committee_members.contains(operator_id)
    }

    /// Justify the round change quorum
    /// In order to justify a round change quorum, we find the maximum round of the quorum set that
    /// had achieved a past consensus. If we have also seen consensus on this round for the
    /// suggested data, then it is justified and this function returns that data.
    /// If there is no past consensus data in the round change quorum or we disagree with quorum set
    /// this function will return None, and we obtain the data as if we were beginning this
    /// instance.
    fn justify_round_change_quorum(&self) -> Option<&ValidatedData<D>> {
        // If we have messages for the current round
        if let Some(new_round_messages) = self.round_change_messages.get(&self.current_round) {
            // If we have a quorum
            if new_round_messages.len() >= self.config.quorum_size {
                // Find the maximum round,value pair
                let max_consensus_data = new_round_messages
                    .values()
                    .max_by_key(|maybe_past_consensus_data| {
                        maybe_past_consensus_data
                            .as_ref()
                            .map(|consensus_data| *consensus_data.round)
                            .unwrap_or(0)
                    })?
                    .clone()?;

                // We a maximum, check to make sure we have seen quorum on this
                let past_data = self.past_consensus.get(&max_consensus_data.round)?;
                if *past_data == max_consensus_data.data {
                    return Some(past_data);
                }
            }
        }
        None
    }

    // Handles the beginning of a round.
    fn start_round(&mut self) {
        debug!(round = *self.current_round, "Starting new round",);

        // Remove round change messages that would be for previous rounds
        self.round_change_messages
            .retain(|&round, _value| round >= self.current_round);

        // Initialise the instance state for the round
        self.state = InstanceState::AwaitingProposal;

        // Check if we are the leader
        if self.check_leader(&self.operator_id()) {
            // We are the leader
            debug!("Current leader");
            // Check justification of round change quorum
            if let Some(validated_data) = self.justify_round_change_quorum().cloned() {
                debug!(
                    old_data = ?validated_data,
                    "Using consensus data from a previous round");
                self.send_proposal(validated_data.clone());
                self.send_prepare(validated_data);
            } else {
                debug!("Using initialised data");
                self.send_proposal(self.start_data.clone());
                self.send_prepare(self.start_data.clone());
            }
        }
    }

    /// We have received a proposal message
    fn received_propose(&mut self, operator_id: OperatorId, consensus_data: ConsensusData<D>) {
        // Check if proposal is from the leader we expect
        if !(self.check_leader(&operator_id)) {
            warn!(from = *operator_id, "PROPOSE message from non-leader");
            return;
        }
        // Check that this operator is in our committee
        if !self.check_committee(&operator_id) {
            warn!(
                from = *operator_id,
                "PROPOSE message from non-committee operator"
            );
            return;
        }

        // Check that we are awaiting a proposal
        if !matches!(self.state, InstanceState::AwaitingProposal) {
            warn!(from=*operator_id, ?self.state, "PROPOSE message while in invalid state");
            return;
        }
        //  Ensure that this message is for the correct round
        if !(self.current_round == consensus_data.round) {
            warn!(
                from = *operator_id,
                current_round = *self.current_round,
                propose_round = *consensus_data.round,
                "PROPOSE message received for the wrong round"
            );
            return;
        }

        // Validate the data
        let Ok(consensus_data) = validate_consensus_data(consensus_data) else {
            warn!(
                from = *operator_id,
                current_round = *self.current_round,
                "PROPOSE message is invalid"
            );
            return;
        };

        debug!(from = *operator_id, "PROPOSE received");

        // Justify the proposal by checking the round changes
        if let Some(justified_data) = self.justify_round_change_quorum() {
            if *justified_data != consensus_data.data {
                // The data doesn't match the justified value we expect. Drop the message
                warn!(
                    from = *operator_id,
                    ?consensus_data,
                    ?justified_data,
                    "PROPOSE message isn't justified"
                );
                return;
            }
            self.send_prepare(consensus_data.data);
        } else {
            // We have no previous consensus data
            // If of valid type, set data locally then send prepare
            self.send_prepare(consensus_data.data);
        }
    }

    /// We have received a prepare message
    fn received_prepare(&mut self, operator_id: OperatorId, consensus_data: ConsensusData<D>) {
        // Check that this operator is in our committee
        if !self.check_committee(&operator_id) {
            warn!(
                from = *operator_id,
                "PREPARE message from non-committee operator"
            );
            return;
        }

        // Check that we are in the correct state
        if (self.state as u8) >= (InstanceState::SentRoundChange as u8) {
            warn!(from=*operator_id, ?self.state, "PREPARE message while in invalid state");
            return;
        }

        //  Ensure that this message is for the correct round
        if !(self.current_round == consensus_data.round) {
            warn!(
                from = *operator_id,
                current_round = *self.current_round,
                propose_round = *consensus_data.round,
                "PREPARE message received for the wrong round"
            );
            return;
        }

        // Validate the data
        let Ok(consensus_data) = validate_consensus_data(consensus_data) else {
            warn!(
                from = *operator_id,
                current_round = *self.current_round,
                "PREPARE message is invalid"
            );
            return;
        };

        debug!(from = *operator_id, "PREPARE received");

        // Store the prepare message
        if !self
            .prepare_messages
            .entry(consensus_data.round)
            .or_default()
            .entry(consensus_data.data)
            .or_default()
            .insert(operator_id)
        {
            warn!(from = *operator_id, "PREPARE message is a duplicate")
        };

        // Check if we have reached quorum, if so send commit messages and store the fact that we
        // have reached consensus on this quorum.
        let mut update_data = None;
        if let Some(prepare_messages) = self.prepare_messages.get(&self.current_round) {
            // Check the quorum size
            if let Some((data, operators)) = prepare_messages
                .iter()
                .max_by_key(|(_data, operators)| operators.len())
            {
                if operators.len() >= self.config.quorum_size
                    && matches!(self.state, InstanceState::Prepare)
                {
                    // We reached quorum on this data
                    update_data = Some(data.clone());
                }
            }
        }

        // Send the data
        if let Some(data) = update_data {
            self.send_commit(data.clone());
            self.insert_consensus(self.current_round, data.clone());
        }
    }

    ///We have received a commit message
    fn received_commit(&mut self, operator_id: OperatorId, consensus_data: ConsensusData<D>) {
        // Check that this operator is in our committee
        if !self.check_committee(&operator_id) {
            warn!(
                from = *operator_id,
                "COMMIT message from non-committee operator"
            );
            return;
        }

        // Check that we are awaiting a proposal
        if (self.state as u8) >= (InstanceState::SentRoundChange as u8) {
            warn!(from=*operator_id, ?self.state, "COMMIT message while in invalid state");
            return;
        }

        //  Ensure that this message is for the correct round
        if !(self.current_round == consensus_data.round) {
            warn!(
                from = *operator_id,
                current_round = *self.current_round,
                propose_round = *consensus_data.round,
                "COMMIT message received for the wrong round"
            );
            return;
        }

        // Validate the data
        let Ok(consensus_data) = validate_consensus_data(consensus_data) else {
            warn!(
                from = *operator_id,
                current_round = *self.current_round,
                "COMMIT message is invalid"
            );
            return;
        };

        debug!(from = *operator_id, "COMMIT received");

        // Store the received commit message
        if !self
            .commit_messages
            .entry(self.current_round)
            .or_default()
            .entry(consensus_data.data)
            .or_default()
            .insert(operator_id)
        {
            warn!(from = *operator_id, "Received duplicate commit");
        }

        // Check if we have reached quorum
        if let Some(commit_messages) = self.commit_messages.get(&self.current_round) {
            // Check the quorum size
            if let Some((data, operators)) = commit_messages
                .iter()
                .max_by_key(|(_data, operators)| operators.len())
            {
                if operators.len() >= self.config.quorum_size
                    && matches!(self.state, InstanceState::Commit)
                {
                    self.send_completed(Completed::Success(data.data.clone()));
                    self.state = InstanceState::Complete;
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
        // Check that this operator is in our committee
        if !self.check_committee(&operator_id) {
            warn!(
                from = *operator_id,
                "ROUNDCHANGE message from non-committee operator"
            );
            return;
        }

        // Check that we are awaiting a proposal
        // NOTE: THis is not necessary, but putting it here as these functions can be grouped for
        // later
        if (self.state as u8) >= (InstanceState::Complete as u8) {
            warn!(from=*operator_id, ?self.state, "ROUNDCHANGE message while in invalid state");
            return;
        }

        //  Ensure that this message is for the correct round
        if round < self.current_round || *round > self.config.max_rounds {
            warn!(
                from = *operator_id,
                current_round = *self.current_round,
                propose_round = *round,
                max_rounds = self.config.max_rounds,
                "ROUNDCHANGE message received for the wrong round"
            );
            return;
        }

        // Validate the data, if it exists
        let maybe_past_consensus_data = match maybe_past_consensus_data {
            Some(consensus_data) => {
                let Ok(consensus_data) = validate_consensus_data(consensus_data) else {
                    warn!(
                        from = *operator_id,
                        current_round = *self.current_round,
                        "ROUNDCHANGE message is invalid"
                    );
                    return;
                };
                Some(consensus_data)
            }
            None => None,
        };

        debug!(from = *operator_id, "ROUNDCHANGE received");

        // Store the round change message, for the round the message references
        if self
            .round_change_messages
            .entry(round)
            .or_default()
            .insert(operator_id, maybe_past_consensus_data.clone())
            .is_some()
        {
            warn!(from = *operator_id, "ROUNDCHANGE duplicate request",);
        }

        // There are two cases to check here
        // 1. If we receive f+1 round change messages, we need to send our own round-change message
        // 2. If we have received a quorum of round change messages, we need to start a new round

        // Check if we have any messages for the suggested round
        if let Some(new_round_messages) = self.round_change_messages.get(&round) {
            // Check the quorum size
            if new_round_messages.len() >= self.config.quorum_size
                && matches!(self.state, InstanceState::SentRoundChange)
            {
                // 1. If we have reached a quorum for this round, advance to that round.
                debug!(operator_id = ?self.operator_id(), round = *round, "Round change quorum reached");
                self.set_round(round);
            } else if new_round_messages.len() > self.get_f()
                && !(matches!(self.state, InstanceState::SentRoundChange))
            {
                // 2. We have seen 2f + 1 messtages for this round.
                self.send_round_change(round);
            }
        }
    }

    // Send message functions
    fn send_proposal(&mut self, data: ValidatedData<D>) {
        self.send_message(OutMessage::Propose(ConsensusData {
            round: self.current_round,
            data: data.data,
        }));
        self.state = InstanceState::Prepare;
        debug!(?self.state, "State Changed");
    }

    fn send_prepare(&mut self, data: ValidatedData<D>) {
        let consensus_data = ConsensusData {
            round: self.current_round,
            data,
        };
        self.send_message(OutMessage::Prepare(consensus_data.clone().into()));
        // And store a prepare locally
        let operator_id = self.operator_id();
        self.prepare_messages
            .entry(self.current_round)
            .or_default()
            .entry(consensus_data.data)
            .or_default()
            .insert(operator_id);

        self.state = InstanceState::Prepare;
        debug!(?self.state, "State Changed");
    }

    fn send_commit(&mut self, data: ValidatedData<D>) {
        let consensus_data = ConsensusData {
            round: self.current_round,
            data,
        };
        self.send_message(OutMessage::Commit(consensus_data.clone().into())); //And store a commit locally
        let operator_id = self.operator_id();
        self.commit_messages
            .entry(self.current_round)
            .or_default()
            .entry(consensus_data.data)
            .or_default()
            .insert(operator_id);
        self.state = InstanceState::Commit;
        debug!(?self.state, "State changed", );
    }

    fn send_round_change(&mut self, round: Round) {
        // Get the maximum round we have come to consensus on
        let best_consensus = self
            .past_consensus
            .iter()
            .max_by_key(|(&round, _v)| *round)
            .map(|(&round, data)| ConsensusData {
                round,
                data: data.clone(),
            });

        self.send_message(OutMessage::RoundChange(
            round,
            best_consensus.clone().map(|v| v.into()),
        ));

        // And store locally
        let operator_id = self.operator_id();
        self.round_change_messages
            .entry(round)
            .or_default()
            .insert(operator_id, best_consensus);

        self.state = InstanceState::SentRoundChange;
        debug!(state = ?self.state, "New State");
    }

    fn send_completed(&mut self, completion_status: Completed<D>) {
        self.send_message(OutMessage::Completed(completion_status));
        self.state = InstanceState::Complete;
        debug!(state = ?self.state, "New State");
    }
}
