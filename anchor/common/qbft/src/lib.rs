use crate::msg_container::MessageContainer;
use ssv_types::consensus::{QbftData, QbftMessage, QbftMessageType, UnsignedSSVMessage};
use ssv_types::message::{MessageID, MsgType, SSVMessage, SignedSSVMessage};
use ssv_types::OperatorId;
use ssz::{Decode, Encode};
use std::collections::HashMap;
use tracing::{debug, error, warn};
use types::Hash256;

// Re-Exports for Manager
pub use config::{Config, ConfigBuilder};
pub use error::ConfigBuilderError;
pub use qbft_types::Message;
pub use qbft_types::WrappedQbftMessage;
pub use qbft_types::{
    Completed, ConsensusData, DefaultLeaderFunction, InstanceHeight, InstanceState, LeaderFunction,
    Round,
};

mod config;
mod error;
mod msg_container;
mod qbft_types;

#[cfg(test)]
mod tests;

// Internal structure to hold the data that is to be included in a new outgoing message
struct MessageData<D: QbftData<Hash = Hash256>> {
    data_round: u64,
    round: u64,
    root: D::Hash,
    full_data: Vec<u8>,
}

impl<D> MessageData<D>
where
    D: QbftData<Hash = Hash256>,
{
    pub fn new(data_round: u64, round: u64, root: D::Hash, full_data: Vec<u8>) -> Self {
        Self {
            data_round,
            round,
            root,
            full_data,
        }
    }
}

/// The structure that defines the Quorum Based Fault Tolerance (QBFT) instance.
///
/// This builds and runs an entire QBFT process until it completes. It can complete either
/// successfully (i.e that it has successfully come to consensus, or through a timeout where enough
/// round changes have elapsed before coming to consensus.
///
/// The QBFT instance will receive WrappedQbftMessages from the network and it will construct
/// UnsignedSSVMessages to be signed and sent on the network.
pub struct Qbft<F, D, S>
where
    F: LeaderFunction + Clone,
    D: QbftData<Hash = Hash256>,
    S: FnMut(Message),
{
    /// The initial configuration used to establish this instance of QBFT.
    config: Config<F>,
    /// The identification of this QBFT instance
    identifier: MessageID,
    /// The instance height acts as an ID for the current instance and helps distinguish it from
    /// other instances.
    instance_height: InstanceHeight,
    /// Hash of the start data
    start_data_hash: D::Hash,
    /// Initial data that we will propose if we are the leader.
    start_data: D,
    /// All of the data that we have seen
    data: HashMap<D::Hash, D>,
    /// The current round this instance state is in.a
    current_round: Round,
    /// The current state of the instance
    state: InstanceState,
    /// If this QBFT instance has been completed, the completed value
    completed: Option<Completed<D::Hash>>,

    // Message containers
    propose_container: MessageContainer,
    prepare_container: MessageContainer,
    commit_container: MessageContainer,
    round_change_container: MessageContainer,

    // Current round state
    proposal_accepted_for_current_round: bool,
    proposal_root: Option<D::Hash>,
    last_prepared_round: Option<Round>,
    last_prepared_value: Option<D::Hash>,

    /// Past prepare consensus that we have reached
    past_consensus: HashMap<Round, D::Hash>,

    // Network sender
    send_message: S,
}

impl<F, D, S> Qbft<F, D, S>
where
    F: LeaderFunction + Clone,
    D: QbftData<Hash = Hash256>,
    S: FnMut(Message),
{
    // Construct a new QBFT Instance and start the first round
    pub fn new(config: Config<F>, start_data: D, send_message: S) -> Self {
        let instance_height = *config.instance_height();
        let current_round = config.round();
        let quorum_size = config.quorum_size();

        let mut qbft = Qbft {
            config,
            identifier: MessageID::new([0; 56]),
            instance_height,

            start_data_hash: start_data.hash(),
            start_data,
            data: HashMap::new(),
            current_round,
            state: InstanceState::AwaitingProposal,
            completed: None,

            propose_container: MessageContainer::new(quorum_size),
            prepare_container: MessageContainer::new(quorum_size),
            commit_container: MessageContainer::new(quorum_size),
            round_change_container: MessageContainer::new(quorum_size),

            proposal_accepted_for_current_round: false,
            proposal_root: None,
            last_prepared_round: None,
            last_prepared_value: None,

            past_consensus: HashMap::new(),

            send_message,
        };
        qbft.data
            .insert(qbft.start_data_hash, qbft.start_data.clone());
        qbft.start_round();
        qbft
    }

    // Hash of the start data
    pub fn start_data_hash(&self) -> &D::Hash {
        &self.start_data_hash
    }

    /// Return a reference to the qbft configuration
    pub fn config(&self) -> &Config<F> {
        &self.config
    }

    // Shifts this instance into a new round>
    fn set_round(&mut self, new_round: Round) {
        self.current_round.set(new_round);
        self.start_round();
    }

    // Validation and check functions.
    fn check_leader(&self, operator_id: &OperatorId) -> bool {
        self.config.leader_fn().leader_function(
            operator_id,
            self.current_round,
            self.instance_height,
            self.config.committee_members(),
        )
    }

    /// Checks to make sure any given operator is in this instance's comittee.
    fn check_committee(&self, operator_id: &OperatorId) -> bool {
        self.config.committee_members().contains(operator_id)
    }

    // Perform base QBFT relevant message verification. This verfiication is applicable to all QBFT
    // message types
    fn validate_message(&self, wrapped_msg: &WrappedQbftMessage) -> bool {
        // Validate the wrapped message. This will validate the SignedSsvMessage and the QbftMessage
        if !wrapped_msg.validate() {
            warn!("Message validation unsuccessful");
            return false;
        }

        // Ensure that this message is for the correct round
        let current_round = self.current_round.get();
        if (wrapped_msg.qbft_message.round < current_round as u64)
            || (wrapped_msg.qbft_message.round > self.config.max_rounds() as u64)
        {
            warn!(
                propose_round = wrapped_msg.qbft_message.round,
                current_round = *self.current_round,
                "Message received for a invalid round"
            );
            return false;
        }

        // Make sure there is only one signer
        if wrapped_msg.signed_message.operator_ids().len() != 1 {
            warn!(
                num_signers = wrapped_msg.signed_message.operator_ids().len(),
                "Propose message only allows one signer"
            );
            return false;
        }

        // Make sure the one signer is in our committee
        let signer = OperatorId(
            *wrapped_msg
                .signed_message
                .operator_ids()
                .first()
                .expect("Confirmed to exist"),
        );
        if !self.check_committee(&signer) {
            warn!("Signer is not part of committee");
            return false;
        }

        // Make sure we are at the correct instance height
        if wrapped_msg.qbft_message.height != *self.instance_height as u64 {
            warn!(
                expected_instance = *self.instance_height,
                "Message received for the wrong instance"
            );
            return false;
        }

        // Fulldata may be empty
        if wrapped_msg.signed_message.full_data().is_empty() {
            return true;
        }

        // Try to decode the data. If we can decode the data, then also validate it
        let data = match D::from_ssz_bytes(wrapped_msg.signed_message.full_data()) {
            Ok(data) => data,
            _ => {
                warn!(in = ?self.config.operator_id(), "Invalid data");
                return false;
            }
        };

        if !data.validate() {
            warn!(in = ?self.config.operator_id(), "Data failed validation");
            return false;
        }

        // Success! Message is well formed
        true
    }

    /// Justify the round change quorum
    /// In order to justify a round change quorum, we find the maximum round of the quorum set that
    /// had achieved a past consensus. If we have also seen consensus on this round for the
    /// suggested data, then it is justified and this function returns that data.
    /// If there is no past consensus data in the round change quorum or we disagree with quorum set
    /// this function will return None, and we obtain the data as if we were beginning this
    /// instance.
    fn justify_round_change_quorum(&self) -> Option<(D::Hash, D)> {
        // Get all round change messages for the current round
        let round_change_messages = self
            .round_change_container
            .get_messages_for_round(self.current_round);

        // If we don't have enough messages for quorum, we can't justify anything
        if round_change_messages.len() < self.config.quorum_size() {
            return None;
        }

        // Find the highest round that any node claims reached preparation
        let highest_prepared = round_change_messages
            .iter()
            .filter(|msg| msg.qbft_message.data_round != 0) // Only consider messages with prepared data
            .max_by_key(|msg| msg.qbft_message.data_round);

        // If we found a message with prepared data
        if let Some(highest_msg) = highest_prepared {
            // Get the prepared data from the message
            let prepared_round = Round::from(highest_msg.qbft_message.data_round);

            // Verify we have also seen this consensus
            if let Some(hash) = self.past_consensus.get(&prepared_round) {
                // We have seen consensus on the data, get the value
                let our_data = self
                    .data
                    .get(hash)
                    .expect("Data must exist since we have seen consensus on it")
                    .clone();
                return Some((*hash, our_data));
            }
        }

        // No consensus found
        None
    }

    // Handles the beginning of a round.
    fn start_round(&mut self) {
        debug!(self=?self.config.operator_id(), round = *self.current_round, "Starting new round");

        // We are waiting for consensus on a round change, do not start the round yet
        if matches!(self.state, InstanceState::SentRoundChange) {
            return;
        }

        // Initialise the instance state for the round
        self.state = InstanceState::AwaitingProposal;

        // Check if we are the leader
        if self.check_leader(&self.config.operator_id()) {
            // We are the leader

            // Check justification of round change quorum. If there is a justification, we will use
            // that data. Otherwise, use the initial state data
            let (data_hash, data) = self
                .justify_round_change_quorum()
                .unwrap_or_else(|| (self.start_data_hash, self.start_data.clone()));

            debug!(operator_id = ?self.config.operator_id(), hash = ?data_hash, data = ?data, "Current leader proposing data");

            // Send the initial proposal and then the following prepare
            self.send_proposal(data_hash, data);
        }
    }

    /// Receive a new message from the network
    pub fn receive(&mut self, wrapped_msg: WrappedQbftMessage) {
        // Perform base qbft releveant verification on the message
        if !self.validate_message(&wrapped_msg) {
            return;
        }

        // We know where is only one signer, so the first (and only) operator in the signed message
        // is the sender
        let operator_id = wrapped_msg
            .signed_message
            .operator_ids()
            .first()
            .expect("Confirmed to exist in validation");
        let operator_id = OperatorId(*operator_id);

        // Check that this sender is in our committee
        if !self.check_committee(&operator_id) {
            warn!(
                from = ?operator_id,
                "PROPOSE message from non-committee operator"
            );
            return;
        }
        let msg_round: Round = wrapped_msg.qbft_message.round.into();

        // All basic verification successful! Dispatch to the correct handler
        match wrapped_msg.qbft_message.qbft_message_type {
            QbftMessageType::Proposal => self.received_propose(operator_id, msg_round, wrapped_msg),
            QbftMessageType::Prepare => self.received_prepare(operator_id, msg_round, wrapped_msg),
            QbftMessageType::Commit => self.received_commit(operator_id, msg_round, wrapped_msg),
            QbftMessageType::RoundChange => {
                self.received_round_change(operator_id, msg_round, wrapped_msg)
            }
        }
    }

    // We have received a new Proposal messaage
    fn received_propose(
        &mut self,
        operator_id: OperatorId,
        round: Round,
        wrapped_msg: WrappedQbftMessage,
    ) {
        // Make sure that we are actually waiting for a proposal
        if !matches!(self.state, InstanceState::AwaitingProposal) {
            warn!(from=?operator_id, self=?self.config.operator_id(), ?self.state, "PROPOSE message while in invalid state");
            return;
        }

        // Check if proposal is from the leader we expect
        if !self.check_leader(&operator_id) {
            warn!(from = ?operator_id, self=?self.config.operator_id(), "PROPOSE message from non-leader");
            return;
        }

        // If we are passed the first round, make sure that the justifications actually justify the
        // received proposal
        if round > Round::default() && !self.validate_justifications(&wrapped_msg) {
            warn!(from = ?operator_id, self=?self.config.operator_id(), "Justification verifiction failed");
            return;
        }

        // We have previously verified that this data is able to be de-serialized. Store it now
        let data = D::from_ssz_bytes(wrapped_msg.signed_message.full_data())
            .expect("Data has already been validated");

        // Verify that the data root matches what was in the message
        let data_hash = data.hash();
        if data.hash() != wrapped_msg.qbft_message.root {
            warn!(from = ?operator_id, self=?self.config.operator_id(), "Data roots do not match");
            return;
        }

        self.data.insert(data_hash, data);

        debug!(from = ?operator_id, in = ?self.config.operator_id(), state = ?self.state, "PROPOSE received");

        // Store the received propse message
        if !self
            .propose_container
            .add_message(round, operator_id, &wrapped_msg)
        {
            warn!(from = ?operator_id, "PROPOSE message is a duplicate");
            return;
        }

        // Update state
        self.proposal_accepted_for_current_round = true;
        self.proposal_root = Some(data_hash);
        self.state = InstanceState::Prepare;
        debug!(in = ?self.config.operator_id(), state = ?self.state, "State updated to PREPARE");

        // Create and send prepare message
        self.send_prepare(wrapped_msg.qbft_message.root);
    }

    // Validate the round change and prepare justifications. Returns true if the justifications
    // correctly justify the proposal
    //
    // A QBFT Message contains fields to a list of round change justifications and prepare
    // justifications. We must go through each of these individually and verify the validity of each
    // one
    fn validate_justifications(&self, msg: &WrappedQbftMessage) -> bool {
        // Record if any of the round change messages have a value that was prepared
        let mut previously_prepared = false;
        let mut max_prepared_round = 0;
        let mut max_prepared_msg = None;

        // Make sure we have a quorum of round change messages
        if msg.qbft_message.round_change_justification.len() < self.config.quorum_size() {
            warn!("Did not receive a quorum of round change messages");
            return false;
        }

        // There was a quorum of round change justifications. We need to go though and verify each
        // one. Each will be a SignedSSVMessage
        for signed_round_change in &msg.qbft_message.round_change_justification {
            // The qbft message is represented as a Vec<u8> in the signed message, deserialize this
            // into a proper QbftMessage
            let round_change: QbftMessage =
                match QbftMessage::from_ssz_bytes(signed_round_change.ssv_message().data()) {
                    Ok(data) => data,
                    Err(_) => return false,
                };

            // Make sure this is actually a round change message
            if !matches!(round_change.qbft_message_type, QbftMessageType::RoundChange) {
                warn!(message_type = ?round_change.qbft_message_type, "Message is not a ROUNDCHANGE message");
                return false;
            }

            // Convert to a wrapped message and perform verification
            let wrapped = WrappedQbftMessage {
                signed_message: signed_round_change.clone(),
                qbft_message: round_change.clone(),
            };
            if !self.validate_message(&wrapped) {
                warn!("ROUNDCHANGE message validation failed");
                return false;
            }

            // If the data_round > 1, that means we have prepared a value in previous rounds
            if round_change.data_round > 1 {
                previously_prepared = true;

                // also track the max prepared value and round
                if round_change.data_round > max_prepared_round {
                    max_prepared_round = round_change.data_round;
                    max_prepared_msg = Some(round_change);
                }
            }
        }

        // If there was a value that was also previously prepared, we must also verify all of the
        // prepare justifications
        if previously_prepared {
            // Make sure we have a quorum of prepare messages
            if msg.qbft_message.prepare_justification.len() < self.config.quorum_size() {
                warn!(
                    num_justifications = msg.qbft_message.prepare_justification.len(),
                    "Not enough prepare messages for quorum"
                );
                return false;
            }

            // Make sure that the roots match
            if msg.qbft_message.root != max_prepared_msg.clone().expect("Confirmed to exist").root {
                warn!("Highest prepared does not match proposed data");
                return false;
            }

            // Validate each prepare message matches highest prepared round/value
            for signed_prepare in &msg.qbft_message.prepare_justification {
                // The qbft message is represented as Vec<u8> in the signed message, deserialize
                // this into a qbft message
                let prepare = match QbftMessage::from_ssz_bytes(signed_prepare.ssv_message().data())
                {
                    Ok(data) => data,
                    Err(_) => return false,
                };

                // Make sure this is a prepare message
                if prepare.qbft_message_type != QbftMessageType::Prepare {
                    warn!("Expected a prepare message");
                    return false;
                }

                let wrapped = WrappedQbftMessage {
                    signed_message: signed_prepare.clone(),
                    qbft_message: prepare.clone(),
                };
                if !self.validate_message(&wrapped) {
                    warn!("PREPARE message validation failed");
                    return false;
                }

                if prepare.root != msg.qbft_message.root {
                    warn!("Proposed data mismatch");
                    return false;
                }
            }
        }
        true
    }

    /// We have received a prepare message
    fn received_prepare(
        &mut self,
        operator_id: OperatorId,
        round: Round,
        wrapped_msg: WrappedQbftMessage,
    ) {
        // Check that we are in the correct state. We do not have to be in the PREPARE state right
        // now as this message may have been delayed
        if (self.state as u8) >= (InstanceState::SentRoundChange as u8) {
            warn!(from=?operator_id, ?self.state, "PREPARE message while in invalid state");
            return;
        }

        // Make sure this is actually a prepare message
        if !(matches!(
            wrapped_msg.qbft_message.qbft_message_type,
            QbftMessageType::Prepare,
        )) {
            warn!(from=?operator_id, self=?self.config.operator_id(), "Expected a PREPARE message");
            return;
        }

        // Make sure that we have accepted a proposal for this round
        if !self.proposal_accepted_for_current_round {
            warn!(from=?operator_id, ?self.state, self=?self.config.operator_id(), "Have not accepted Proposal for current round yet");
            return;
        }

        debug!(from = ?operator_id, self = ?self.config.operator_id(), state = ?self.state, "PREPARE received");

        // Store the prepare message
        if !self
            .prepare_container
            .add_message(round, operator_id, &wrapped_msg)
        {
            warn!(from = ?operator_id, "PREPARE message is a duplicate")
        }

        // Check if we have reached a prepare quorum for this round, if so send the commit message
        if let Some(hash) = self.prepare_container.has_quorum(round) {
            // Make sure we are in the correct state
            if !matches!(self.state, InstanceState::Prepare)
                && !matches!(self.state, InstanceState::AwaitingProposal)
            {
                warn!(from=?operator_id, self=?self.config.operator_id(), ?self.state, "Not in PREPARE state");
                return;
            }

            // Make sure that the root of the data that we have come to a prepare consensus on
            // matches the root of the proposal that we have accepted
            if hash != self.proposal_root.expect("Proposal has been accepted") {
                warn!("PREPARE quorum root does not match accepted PROPOSAL root");
                return;
            }

            // Success! We have come to a prepare consensus on a value

            // Move the state forward since we have a prepare quorum
            self.state = InstanceState::Commit;
            debug!(in = ?self.config.operator_id(), state = ?self.state, "Reached a PREPARE consensus. State updated to COMMIT");

            // Record that we have come to a consensus on this value
            self.past_consensus.insert(round, hash);

            // Record as last prepared value and round
            self.last_prepared_value = Some(hash);
            self.last_prepared_round = Some(self.current_round);

            // Send a commit message for the prepare quorum data
            self.send_commit(hash);
        }
    }

    /// We have received a commit message
    fn received_commit(
        &mut self,
        operator_id: OperatorId,
        round: Round,
        wrapped_msg: WrappedQbftMessage,
    ) {
        // If we are already done, ignore
        if self.completed.is_some() {
            return;
        }

        // Make sure that we are in the correct state
        if (self.state as u8) >= (InstanceState::SentRoundChange as u8) {
            warn!(from=*operator_id, ?self.state, "COMMIT message while in invalid state");
            return;
        }

        // Make sure this is actually a commit message
        if !(matches!(
            wrapped_msg.qbft_message.qbft_message_type,
            QbftMessageType::Commit,
        )) {
            warn!(from=?operator_id, self=?self.config.operator_id(), "Expected a COMMIT message");
            return;
        }

        // Make sure that we have accepted a proposal for this round
        if !self.proposal_accepted_for_current_round {
            warn!(from=?operator_id, ?self.state, self=?self.config.operator_id(), "Have not accepted Proposal for current round yet");
            return;
        }

        debug!(from = ?operator_id, in = ?self.config.operator_id(), state = ?self.state, "COMMIT received");

        // Store the received commit message
        if !self
            .commit_container
            .add_message(round, operator_id, &wrapped_msg)
        {
            warn!(from = ?operator_id, "COMMIT message is a duplicate")
        }

        // Check if we have a commit quorum
        if let Some(hash) = self.commit_container.has_quorum(round) {
            // Make sure that the root of the data that we have come to a commit consensus on
            // matches the root of the proposal that we have accepted
            if hash != self.proposal_root.expect("Proposal has been accepted") {
                warn!("COMMIT quorum root does not match accepted PROPOSAL root");
                return;
            }

            // All validation successful, make sure we are in the proper commit state
            if matches!(self.state, InstanceState::Commit) {
                // Todo!(). Commit aggregation

                // We have come to commit consensus, mark ourself as completed and record the agreed upon
                // value
                self.state = InstanceState::Complete;
                self.completed = Some(Completed::Success(hash));
                debug!(in = ?self.config.operator_id(), state = ?self.state, "Reached a COMMIT consensus. Success!");
            }
        }
    }

    /// We have received a round change message.
    fn received_round_change(
        &mut self,
        operator_id: OperatorId,
        round: Round,
        wrapped_msg: WrappedQbftMessage,
    ) {
        // Make sure we are in the correct state
        if (self.state as u8) >= (InstanceState::Complete as u8) {
            warn!(from=*operator_id, ?self.state, "ROUNDCHANGE message while in invalid state");
            return;
        }

        debug!(from = ?operator_id, in = ?self.config.operator_id(), state = ?self.state, "ROUNDCHANGE received");

        // Store the round changed message
        if !self
            .round_change_container
            .add_message(round, operator_id, &wrapped_msg)
        {
            warn!(from = ?operator_id, "ROUNDCHANGE message is a duplicate")
        }

        // There are two cases to check here

        // 1. If we have received a quorum of round change messages, we need to start a new round
        if self.round_change_container.has_quorum(round).is_some() {
            if matches!(self.state, InstanceState::SentRoundChange) {
                // If we have reached a quorum for this round and have already sent a round change, advance to that round.
                debug!(
                    operator_id = ?self.config.operator_id(),
                    round = *round,
                    "Round change quorum reached"
                );

                // We have reached consensus on a round change, we can start a new round now
                self.state = InstanceState::RoundChangeConsensus;

                // The round change messages is round + 1, so this is the next round we want to use
                self.set_round(round);
            }
        } else {
            // 2. If we receive f+1 round change messages, we need to send our own round-change message
            let num_messages_for_round = self.round_change_container.num_messages_for_round(round);
            if num_messages_for_round > self.config.get_f()
                && !(matches!(self.state, InstanceState::SentRoundChange))
            {
                // Set the state so SendRoundChange so we include Round + 1 in message
                self.state = InstanceState::SentRoundChange;

                self.send_round_change(Hash256::default());
            }
        }
    }

    // End the current round and move to the next one, if possible.
    pub fn end_round(&mut self) {
        debug!(self=?self.config.operator_id(), round = *self.current_round, "Incrementing round");
        let Some(next_round) = self.current_round.next() else {
            self.state = InstanceState::Complete;
            self.completed = Some(Completed::TimedOut);
            return;
        };

        if next_round.get() > self.config.max_rounds() {
            self.state = InstanceState::Complete;
            self.completed = Some(Completed::TimedOut);
            return;
        }

        // Bump the current round
        self.current_round = next_round;

        // Set the state so SendRoundChange so we include Round + 1 in message
        self.state = InstanceState::SentRoundChange;

        self.send_round_change(Hash256::default());
        self.start_round();
    }

    // Get data for the qbft message
    fn get_message_data(&self, msg_type: &QbftMessageType, data_hash: D::Hash) -> MessageData<D> {
        let full_data = if matches!(msg_type, QbftMessageType::Proposal) {
            self.data
                .get(&data_hash)
                .expect("Value exists")
                .as_ssz_bytes()
        } else {
            vec![]
        };

        if matches!(msg_type, QbftMessageType::RoundChange) {
            if let (Some(last_prepared_value), Some(last_prepared_round)) =
                (self.last_prepared_value, self.last_prepared_round)
            {
                return MessageData::new(
                    last_prepared_round.get() as u64,
                    self.current_round.get() as u64,
                    last_prepared_value,
                    self.data
                        .get(&last_prepared_value)
                        .expect("Value exists")
                        .as_ssz_bytes(),
                );
            }
        }

        // Standard message data for Proposal, Prepare, and Commit
        MessageData::new(0, self.current_round.get() as u64, data_hash, full_data)
    }

    // Construct a new unsigned message. This will be passed to the processor to be signed and then
    // sent on the network
    fn new_unsigned_message(
        &self,
        msg_type: QbftMessageType,
        data_hash: D::Hash,
        round_change_justification: Vec<SignedSSVMessage>,
        prepare_justification: Vec<SignedSSVMessage>,
    ) -> UnsignedSSVMessage {
        let data = self.get_message_data(&msg_type, data_hash);

        // Create the QBFT message
        let qbft_message = QbftMessage {
            qbft_message_type: msg_type,
            height: *self.instance_height as u64,
            round: data.round,
            identifier: self.identifier.clone(),
            root: data.root,
            data_round: data.data_round,
            round_change_justification,
            prepare_justification,
        };

        let ssv_message = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            self.identifier.clone(),
            qbft_message.as_ssz_bytes(),
        );

        // Wrap in unsigned SSV message
        UnsignedSSVMessage {
            ssv_message,
            full_data: data.full_data,
        }
    }

    // Get all of the round change jusitifcation messages
    fn get_round_change_justifications(&self) -> Vec<SignedSSVMessage> {
        // Short circuit if we are in first round
        if self.current_round <= Round::default() {
            return vec![];
        }

        // If we are past the first round and awaiting proposal, that means that there was a
        // round change and we must have a quorum of round change messages. We include these so
        // that we can prove that we had a consensus allowing us to change
        if matches!(self.state, InstanceState::AwaitingProposal) {
            return self
                .round_change_container
                .get_messages_for_round(self.current_round)
                .iter()
                .map(|msg| msg.signed_message.clone())
                .collect();
        }
        // If we are past the first round and are sending a round change. We have to include
        // prepare messages that prove we have prepared a value
        else if matches!(self.state, InstanceState::SentRoundChange) {
            // if we have a last prepared value and a last prepared round...
            if let (Some(_), Some(last_prepared_round)) =
                (self.last_prepared_value, self.last_prepared_round)
            {
                // Get all of the prepare messages for the last prepared round
                let last_prepared_messages = self
                    .prepare_container
                    .get_messages_for_round(last_prepared_round);

                // Make sure we have a quorum of prepare message
                if last_prepared_messages.len() < self.config.quorum_size() {
                    return vec![];
                }

                // This will hold the value that we want to propose
                return last_prepared_messages
                    .iter()
                    .map(|msg| msg.signed_message.clone())
                    .collect();
            }
            return vec![];
        }

        // Sending prepare/commit message
        vec![]
    }

    // Get all of the prepare justifications for proposals
    fn get_prepare_justifications(&self) -> (Vec<SignedSSVMessage>, Option<Hash256>) {
        // No justifications if we are in the first round
        if self.current_round <= Round::default() {
            return (vec![], None);
        }

        // We only send prepare justifications with for proposal messages. If we are in the
        // state AwaitingProposal and sending a message, we know this is a proposal. This will
        // happen when we have come to a consensus of round change messages and have started a
        // new round
        if matches!(self.state, InstanceState::AwaitingProposal) {
            // go through all of the prepares for the leading round and see if we have have come
            // to a justification?

            // Get all of the round change messages for the current round and make sure we have
            // a quorum of them.
            let round_change_msg = self
                .round_change_container
                .get_messages_for_round(self.current_round);
            if round_change_msg.len() < self.config.quorum_size() {
                return (vec![], None);
            }

            // Go through each message and see if any have a value that was already prepared
            // Just want to take the first one that is valid and has a prepared value
            for wrapped_round_change in round_change_msg {
                // Deserialize into a qbft message for sanity checks
                let round_change: QbftMessage = match QbftMessage::from_ssz_bytes(
                    wrapped_round_change.signed_message.ssv_message().data(),
                ) {
                    Ok(data) => data,
                    Err(_) => return (vec![], None),
                };

                // Round sanity check
                let current_round_proposal = self.proposal_accepted_for_current_round
                    && self.current_round.get() as u64 == round_change.round;
                let future_round_proposal = round_change.round > self.current_round.get() as u64;
                if !current_round_proposal && !future_round_proposal {
                    continue;
                }

                // Validate the proposal, if this is a valid proposal then this is our prepare
                // justification
                if self.validate_justifications(wrapped_round_change) {
                    return (
                        vec![wrapped_round_change.signed_message.clone()],
                        Some(round_change.root),
                    );
                }
            }
        }

        // Not sending a proposal
        (vec![], None)
    }

    // Send a new qbft proposal message
    fn send_proposal(&mut self, hash: D::Hash, data: D) {
        // Store the data we're proposing
        self.data.insert(hash, data.clone());

        // For Proposal messages
        // round_change_justification: list of round change messages
        let round_change_justifications = self.get_round_change_justifications();
        // prepare_justification: list of prepare messages
        let (prepare_justifications, value_to_propose) = self.get_prepare_justifications();

        // Determine the value that should be proposed based off of justification. If we have a
        // prepare justification, we want to propose that value. Else, just propose the start data
        let value_to_propose = match value_to_propose {
            Some(value) => value,
            None => self.start_data_hash,
        };

        // Construct a unsigned proposal
        let unsigned_msg = self.new_unsigned_message(
            QbftMessageType::Proposal,
            value_to_propose,
            round_change_justifications,
            prepare_justifications,
        );

        let operator_id = self.config.operator_id();
        (self.send_message)(Message::Propose(operator_id, unsigned_msg.clone()));
    }

    // Send a new qbft prepare message
    fn send_prepare(&mut self, data_hash: D::Hash) {
        // Only send prepare if we've seen this data
        if !self.data.contains_key(&data_hash) {
            warn!("Attempted to prepare unknown data");
            return;
        }

        // Construct unsigned prepare
        let unsigned_msg =
            self.new_unsigned_message(QbftMessageType::Prepare, data_hash, vec![], vec![]);

        let operator_id = self.config.operator_id();
        (self.send_message)(Message::Prepare(operator_id, unsigned_msg.clone()));
    }

    // Send a new qbft commit message
    fn send_commit(&mut self, data_hash: D::Hash) {
        // Construct unsigned commit
        let unsigned_msg =
            self.new_unsigned_message(QbftMessageType::Commit, data_hash, vec![], vec![]);

        let operator_id = self.config.operator_id();
        (self.send_message)(Message::Commit(operator_id, unsigned_msg.clone()));
    }

    // Send a new qbft round change message
    fn send_round_change(&mut self, data_hash: D::Hash) {
        // For Round Change messages
        // round_change_justification: list of prepare messages
        let round_change_justifications = self.get_round_change_justifications();
        // prepare_justification: N/A

        // Construct unsigned round change
        let unsigned_msg = self.new_unsigned_message(
            QbftMessageType::RoundChange,
            data_hash,
            round_change_justifications,
            vec![],
        );

        // forget that we accpeted a proposal
        self.proposal_accepted_for_current_round = false;

        let operator_id = self.config.operator_id();
        (self.send_message)(Message::RoundChange(operator_id, unsigned_msg.clone()));
    }

    /// Extract the data that the instance has come to consensus on
    pub fn completed(&self) -> Option<Completed<D>> {
        self.completed
            .clone()
            .and_then(|completed| match completed {
                Completed::TimedOut => Some(Completed::TimedOut),
                Completed::Success(hash) => {
                    let data = self.data.get(&hash).cloned();
                    if data.is_none() {
                        error!("could not find finished data");
                    }
                    data.map(Completed::Success)
                }
            })
    }
}
