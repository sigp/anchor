use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

// Re-Exports for Manager
pub use config::{Config, ConfigBuilder};
pub use error::ConfigBuilderError;
pub use qbft_types::{
    Completed, ConsensusData, DefaultLeaderFunction, InstanceHeight, InstanceState, LeaderFunction,
    UnsignedWrappedQbftMessage, WrappedQbftMessage,
};
use ssv_types::{
    OperatorId, Round,
    consensus::{QbftData, QbftDataValidator, QbftMessage, QbftMessageType, UnsignedSSVMessage},
    message::{MsgType, SSVMessage, SignedSSVMessage},
    msgid::MessageId,
};
use ssz::{Decode, Encode};
use tracing::{debug, error, warn};
use types::Hash256;

use crate::msg_container::MessageContainer;

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

impl<D: QbftData<Hash = Hash256>> MessageData<D> {
    pub fn new(data_round: u64, round: u64, root: D::Hash, full_data: Vec<u8>) -> Self {
        Self {
            data_round,
            round,
            root,
            full_data,
        }
    }
}

// Store hash and deserialized data together to avoid redundant lookups
#[derive(Debug, Default, Clone)]
pub struct ValidData<D: QbftData<Hash = Hash256>> {
    hash: D::Hash,
    data: Option<Arc<D>>,
}

impl<D: QbftData<Hash = Hash256>> ValidData<D> {
    fn new(data: Option<Arc<D>>, hash: Hash256) -> Self {
        Self { hash, data }
    }
}

pub trait MessageSender {
    fn send(&mut self, msg: UnsignedWrappedQbftMessage);
}

impl<T: FnMut(UnsignedWrappedQbftMessage)> MessageSender for T {
    fn send(&mut self, msg: UnsignedWrappedQbftMessage) {
        self(msg)
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
    S: MessageSender,
{
    /// The initial configuration used to establish this instance of QBFT.
    config: Config<F>,
    /// The identification of this QBFT instance
    identifier: MessageId,
    /// The instance height acts as an ID for the current instance and helps distinguish it from
    /// other instances.
    instance_height: InstanceHeight,
    /// Hash of the start data
    start_data_hash: D::Hash,
    /// Initial data that we will propose if we are the leader.
    start_data: Arc<D>,
    /// Validated start data
    valid_start_data: ValidData<D>,
    /// All of the data that we have seen
    data: HashMap<D::Hash, Arc<D>>,
    /// The current round this instance state is in.
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

    /// Aggregated commit message
    aggregated_commit: Option<SignedSSVMessage>,

    /// Message sender callback to instruct managing code to send a message
    message_sender: S,

    data_validator: Box<dyn QbftDataValidator<D>>,
}

impl<F, D, S> Qbft<F, D, S>
where
    F: LeaderFunction + Clone,
    D: QbftData<Hash = Hash256>,
    S: MessageSender,
{
    /// Constructs a new QBFT instance and starts the first round.
    ///
    /// # Parameters
    /// - `config`: The initial configuration used to establish this QBFT instance.
    /// - `start_data`: The initial data that will be proposed if this node is the leader.
    /// - `identifier`: The message identifier for this QBFT instance's outgoing messages.
    /// - `message_sender`: A callback used by the instance to trigger message sending.
    pub fn new(
        config: Config<F>,
        start_data: D,
        data_validator: Box<dyn QbftDataValidator<D>>,
        identifier: MessageId,
        message_sender: S,
    ) -> Self {
        let instance_height = *config.instance_height();
        let current_round = config.round();
        let quorum_size = config.quorum_size();

        let start_data = Arc::new(start_data);
        let start_data_hash = start_data.hash();
        let valid_start_data = ValidData::new(Some(start_data.clone()), start_data_hash);

        let mut qbft = Qbft {
            config,
            identifier,
            instance_height,

            start_data_hash,
            start_data,
            valid_start_data,
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

            aggregated_commit: None,

            message_sender,
            data_validator,
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

    /// Get the current round
    pub fn get_round(&self) -> Round {
        self.current_round
    }

    // Shifts this instance into a new round>
    fn set_round(&mut self, new_round: Round) {
        self.current_round.set(new_round);
        self.start_round();
    }

    // Get the aggregated commit message, if it exists
    pub fn get_aggregated_commit(&self) -> Option<SignedSSVMessage> {
        self.aggregated_commit.clone()
    }

    // Validation and check functions.
    fn check_leader(&self, operator_id: &OperatorId, round: Round) -> bool {
        self.config.leader_fn().leader_function(
            operator_id,
            round,
            self.instance_height,
            self.config.committee_members(),
        )
    }

    /// Checks to make sure any given operator is in this instance's comittee.
    fn check_committee(&self, operator_id: &OperatorId) -> bool {
        self.config.committee_members().contains(operator_id)
    }

    /// Checks if we have a quorum of unique committee operators from these messages.
    fn check_quorum<'a>(&self, msgs: impl IntoIterator<Item = &'a SignedSSVMessage>) -> bool {
        let unique_operators = msgs
            .into_iter()
            .flat_map(|justification| justification.operator_ids())
            .filter(|operator_id| self.check_committee(operator_id))
            .collect::<HashSet<_>>();
        unique_operators.len() >= self.config.quorum_size()
    }

    // Perform base QBFT relevant message verification. This verfiication is applicable to all QBFT
    // message types
    // Return type expresses that we either have
    // 1) An invalid message via None
    // 2) A valid message with empty fulldata via Some(None, ID)
    // 3) A valid message with fulldata via Some(data, ID)
    fn validate_message(
        &self,
        wrapped_msg: &WrappedQbftMessage,
    ) -> Option<(Option<ValidData<D>>, OperatorId)> {
        // Ensure that this message is for the correct round
        if wrapped_msg.qbft_message.round < self.current_round.into() {
            debug!(
                message_round = wrapped_msg.qbft_message.round,
                current_round = *self.current_round,
                "Message received for a previous round"
            );
            return None;
        }

        // Check for future round
        if wrapped_msg.qbft_message.round > self.current_round.into() {
            match wrapped_msg.qbft_message.qbft_message_type {
                QbftMessageType::Proposal | QbftMessageType::RoundChange => {
                    // Proposals & Round Changes for future rounds are always allowed
                }
                QbftMessageType::Commit => {
                    // Only decided messages (with quorum) are allowed from future rounds
                    if wrapped_msg.signed_message.operator_ids().len() < self.config.quorum_size() {
                        return None;
                    }
                }
                _ => {
                    // All other message types (including Prepare) for future rounds are not allowed
                    return None;
                }
            }
        }

        // Make sure we are at the correct instance height
        if wrapped_msg.qbft_message.height != *self.instance_height as u64 {
            warn!(
                expected_instance = *self.instance_height,
                "Message received for the wrong instance"
            );
            return None;
        }

        // Make sure that all of the signers are in our committee
        for signer in wrapped_msg.signed_message.operator_ids() {
            if !self.check_committee(signer) {
                warn!("Signer is not part of committee");
                return None;
            }
        }

        // The rest of the verification only pertains to messages with one signature
        if wrapped_msg.signed_message.operator_ids().len() > 1 {
            // The message validator already checked this is a decided message (a commit message
            // with > 1 signers). Do not care about data here, just that we had a
            // success
            let valid_data = Some(ValidData::new(None, wrapped_msg.qbft_message.root));
            return Some((valid_data, OperatorId::from(0)));
        }

        // Message is not a decide message, we know there is only one signer
        let signer = wrapped_msg.signed_message.operator_ids().first()?;

        // Fulldata may be empty. This is still considered valid though. We also do not validate
        // fulldata on round change messages.
        if wrapped_msg.signed_message.full_data().is_empty()
            || wrapped_msg.qbft_message.qbft_message_type == QbftMessageType::RoundChange
        {
            let valid_data = Some(ValidData::new(None, wrapped_msg.qbft_message.root));
            return Some((valid_data, *signer));
        }

        // Try to decode the data. If we can decode the data, then also validate it
        let data = match D::from_ssz_bytes(wrapped_msg.signed_message.full_data()) {
            Ok(data) => data,
            _ => {
                error!(
                    msg = %wrapped_msg,
                    "Invalid full data received",
                );
                debug!(
                    full_data = hex::encode(wrapped_msg.signed_message.full_data()),
                    "Raw invalid full data",
                );
                return None;
            }
        };

        if !self.data_validator.validate(&data, &self.start_data) {
            return None;
        }

        // Success! Message is well formed
        let valid_data = Some(ValidData::new(
            Some(Arc::new(data)),
            wrapped_msg.qbft_message.root,
        ));
        Some((valid_data, *signer))
    }

    /// Justify the round change quorum
    /// Finds the highest prepared value from round change messages and returns it
    /// for the proposal.
    fn justify_round_change_quorum(&self) -> Option<ValidData<D>> {
        let round_change_messages = self
            .round_change_container
            .get_messages_for_round(self.current_round);

        // Need quorum to proceed
        if round_change_messages.len() < self.config.quorum_size() {
            return None;
        }

        // Find the round change with the highest prepared round
        let highest_prepared = round_change_messages
            .iter()
            .filter(|msg| msg.qbft_message.data_round != 0)
            .max_by_key(|msg| msg.qbft_message.data_round);

        // If no one prepared anything, return None (will use start data)
        let highest_prepared = highest_prepared?;

        let claimed_hash = highest_prepared.qbft_message.root;

        // First, try to get data from the round change message itself
        if highest_prepared.signed_message.full_data().is_empty() {
            return None;
        }

        // The round change includes the full data - decode and use it
        let Ok(data) = D::from_ssz_bytes(highest_prepared.signed_message.full_data()) else {
            warn!("Failed to decode round change full data");
            return None;
        };

        // Verify the data matches the claimed hash
        if data.hash() != claimed_hash {
            warn!("Round change full data doesn't match claimed hash");
            return None;
        }

        if !self.data_validator.validate(&data, &self.start_data) {
            warn!("Round change full data is invalid");
            return None;
        }

        Some(ValidData::new(Some(Arc::new(data)), claimed_hash))
    }

    // Handles the beginning of a round.
    fn start_round(&mut self) {
        // We are waiting for consensus on a round change, do not start the round yet
        if matches!(self.state, InstanceState::SentRoundChange) {
            return;
        }

        debug!(round = *self.current_round, "Starting new round");

        // Initialise the instance state for the round
        self.state = InstanceState::AwaitingProposal;

        // Check if we are the leader
        if self.check_leader(&self.config.operator_id(), self.current_round) {
            // We are the leader

            // Check justification of round change quorum. If there is a justification, we will use
            // that data. Otherwise, use the initial state data
            let valid_data = self
                .justify_round_change_quorum()
                .unwrap_or_else(|| self.valid_start_data.clone());

            debug!(hash = ?valid_data.hash, "Current leader proposing data");

            // Send the initial proposal and then the following prepare
            self.send_proposal(valid_data.hash, valid_data.data.expect("Start data exists"));
        }
    }

    /// Receive a new message from the network
    pub fn receive(&mut self, wrapped_msg: WrappedQbftMessage) {
        // Perform base qbft releveant verification on the message
        let Some((Some(valid_data), signer)) = self.validate_message(&wrapped_msg) else {
            return;
        };

        let msg_round: Round = wrapped_msg.qbft_message.round.into();

        // All basic verification successful! Dispatch to the correct handler
        match wrapped_msg.qbft_message.qbft_message_type {
            QbftMessageType::Proposal => {
                self.received_propose(valid_data, signer, msg_round, wrapped_msg)
            }
            QbftMessageType::Prepare => self.received_prepare(signer, msg_round, wrapped_msg),
            QbftMessageType::Commit => {
                if wrapped_msg.signed_message.operator_ids().len() == 1 {
                    self.received_commit(signer, msg_round, wrapped_msg)
                } else {
                    self.received_decided(wrapped_msg)
                }
            }
            QbftMessageType::RoundChange => {
                self.received_round_change(signer, msg_round, wrapped_msg)
            }
        }
    }

    // We have received a new Proposal messaage
    fn received_propose(
        &mut self,
        valid_data: ValidData<D>,
        operator_id: OperatorId,
        round: Round,
        wrapped_msg: WrappedQbftMessage,
    ) {
        // Make sure that we are actually waiting for a proposal
        if round == self.current_round && !matches!(self.state, InstanceState::AwaitingProposal) {
            debug!(from=?operator_id, ?self.state, "PROPOSE message while in invalid state");
            return;
        }

        // Make sure this is from the leader
        if !self.check_leader(&operator_id, round) {
            warn!(from = ?operator_id, "PROPOSE message received from non-leader operator");
            return;
        }

        // If we are passed the first round, make sure that the justifications actually justify the
        // received proposal
        if round > Round::default() {
            // validate the justifications
            if !self.validate_proposal_justifications(&wrapped_msg) {
                warn!(from = ?operator_id, "Justification validation failed for proposal");
                return;
            }
        }

        // Fulldata is included in propose messages
        let data = match valid_data.data {
            Some(data) => data,
            None => {
                warn!(from = ?operator_id, "Proposal should contain data");
                return;
            }
        };
        self.data.insert(valid_data.hash, data);

        debug!(from = ?operator_id, state = ?self.state, "PROPOSE received");

        // Store the received propse message
        if !self
            .propose_container
            .add_message(round, operator_id, &wrapped_msg)
        {
            warn!(from = ?operator_id, "PROPOSE message is a duplicate");
            return;
        }

        // Only reject if we've already accepted a proposal for THIS round
        // Allow proposals for future rounds even if we have a proposal for current round
        if self.proposal_accepted_for_current_round && round == self.current_round {
            warn!(from = ?operator_id, "Proposal has already been accepted for this round");
            return;
        }

        // If this is a future round proposal, update our round to match
        if round > self.current_round {
            debug!(old_round = ?self.current_round, new_round = ?round, "Updating to future round from proposal");
            self.current_round = round;
        }

        // Accept this proposal
        self.proposal_accepted_for_current_round = true;
        self.proposal_root = Some(valid_data.hash);
        self.state = InstanceState::Prepare {
            proposal_root: valid_data.hash,
        };
        debug!(state = ?self.state, "State updated to PREPARE");

        // Create and send prepare message
        self.send_prepare(wrapped_msg.qbft_message.root);
    }

    // Validate the round change and prepare justifications for proposal.
    // Returns true if the justifications correctly justify the proposal
    //
    // A QBFT Message contains fields to a list of round change justifications and prepare
    // justifications. We must go through each of these individually and verify the validity of each
    // one
    //
    // Proposal
    // - round change justifications
    //  - list of round change messages
    //      - each round change message has list of prepare messages if it prepared a value
    // - prepare justifications
    //  - list of prepare messages to
    fn validate_proposal_justifications(&self, msg: &WrappedQbftMessage) -> bool {
        // Record if any of the round change messages have a value that was prepared
        let mut max_prepared_round = 0;
        let mut max_prepared_msg = None;

        // Make sure we have a quorum of round change messages
        if !self.check_quorum(&msg.qbft_message.round_change_justification) {
            warn!("Did not receive a quorum of round change messages");
            return false;
        }

        // There was a quorum of round change justifications. We need to go though and verify each
        // one. Each will be a SignedSSVMessage
        for signed_round_change in &msg.qbft_message.round_change_justification {
            // Check for multi-signers - round change messages should only have 1 signer
            if signed_round_change.operator_ids().len() != 1
                || signed_round_change.signatures().len() != 1
            {
                return false;
            }

            // make sure all signers in committee
            for signer in signed_round_change.operator_ids() {
                if !self.check_committee(signer) {
                    return false;
                }
            }

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

            // make sure the round change matches the round of the message
            if round_change.round != msg.qbft_message.round {
                return false;
            }

            // For round change justifications, we need special validation that doesn't check
            // against current round since they're justifications from the proposal's round
            // Check height
            if round_change.height != *self.instance_height as u64 {
                return false;
            }

            // If the data_round > 0, that means we have prepared a value in previous rounds
            // We also have to go through all of the prepare justifications in the round change to
            // ensure that they are well formed and properly justify the prepared value
            if round_change.data_round > 0 {
                // also track the max prepared value and round
                if round_change.data_round > max_prepared_round {
                    max_prepared_round = round_change.data_round;
                    max_prepared_msg = Some(round_change.clone());
                }

                // Check that prepared round is not greater than current round
                if round_change.data_round > round_change.round {
                    warn!(
                        "Round change has prepared round {} > round {}",
                        round_change.data_round, round_change.round
                    );
                    return false;
                }

                // Verify that if round change has full data, it matches the root
                if msg.qbft_message.root != round_change.root {
                    warn!("Proposal root doesn't match round change prepared root");
                    return false;
                }

                if !self.check_quorum(&round_change.round_change_justification) {
                    warn!(
                        num_justifications = round_change.round_change_justification.len(),
                        "Not enough prepare messages for quorum"
                    );
                    return false;
                }

                // go through all of the round changes prepare justifications
                for signed_prepare in &round_change.round_change_justification {
                    if !self.is_valid_prepare_justification_for_round_and_root(
                        signed_prepare,
                        round_change.data_round.into(),
                        &round_change.root,
                    ) {
                        return false;
                    }
                }
            }
        }

        // If there was a value that was also previously prepared, we must also verify all of the
        // prepare justifications
        if let Some(max_prepared_msg) = max_prepared_msg {
            // Make sure we have a quorum of prepare messages
            if !self.check_quorum(&msg.qbft_message.prepare_justification) {
                warn!(
                    num_justifications = msg.qbft_message.prepare_justification.len(),
                    "Not enough prepare messages for quorum"
                );
                return false;
            }

            // Make sure that the roots match
            if msg.qbft_message.root != max_prepared_msg.root {
                warn!("Highest prepared does not match proposed data");
                return false;
            }

            // Validate each prepare message matches highest prepared round/value
            for signed_prepare in &msg.qbft_message.prepare_justification {
                if !self.is_valid_prepare_justification_for_round_and_root(
                    signed_prepare,
                    max_prepared_msg.data_round.into(),
                    &max_prepared_msg.root,
                ) {
                    return false;
                }
            }
        }
        true
    }

    fn is_valid_prepare_justification_for_round_and_root(
        &self,
        justification: &SignedSSVMessage,
        round: Round,
        root: &Hash256,
    ) -> bool {
        // Make sure there is only one signer
        let [operator_id] = justification.operator_ids()[..] else {
            return false;
        };
        if justification.signatures().len() != 1 {
            return false;
        }

        // Make sure the signer is in our committee
        if !self.check_committee(&operator_id) {
            return false;
        }

        // The qbft message is represented as Vec<u8> in the signed message, deserialize this into
        // a qbft message
        let Ok(prepare) = QbftMessage::from_ssz_bytes(justification.ssv_message().data()) else {
            warn!("Failed to decode prepare justification message");
            return false;
        };

        // Make sure this is a prepare message
        if prepare.qbft_message_type != QbftMessageType::Prepare {
            warn!("Expected a prepare message");
            return false;
        }

        if prepare.height != *self.instance_height as u64 {
            warn!("PREPARE height incorrect");
            return false;
        }

        if prepare.round != round.get() as u64 {
            warn!("PREPARE round incorrect");
            return false;
        }

        if &prepare.root != root {
            warn!("Proposed data mismatch");
            return false;
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
        // If we are already done
        if self.completed.is_some() {
            return;
        }

        // Check that we are in the correct state. We do not have to be in the PREPARE state right
        // now as this message may have been delayed
        if u8::from(self.state) >= u8::from(InstanceState::SentRoundChange) {
            debug!(from=?operator_id, ?self.state, "PREPARE message while in invalid state");
            return;
        }

        // Make sure this is actually a prepare message
        if !(matches!(
            wrapped_msg.qbft_message.qbft_message_type,
            QbftMessageType::Prepare,
        )) {
            warn!(from=?operator_id, "Expected a PREPARE message");
            return;
        }

        debug!(from = ?operator_id, state = ?self.state, "PREPARE received");

        // Store the prepare message
        if !self
            .prepare_container
            .add_message(round, operator_id, &wrapped_msg)
        {
            warn!(from = ?operator_id, "PREPARE message is a duplicate")
        }

        // Make sure that we have accepted a proposal for this round
        if !self.proposal_accepted_for_current_round {
            debug!(from=?operator_id, ?self.state, "Have not accepted Proposal for current round yet");
            return;
        }

        // Check that the prepare message is for the accepted proposal
        if let Some(accepted_root) = self.proposal_root
            && wrapped_msg.qbft_message.root != accepted_root
        {
            warn!(from=?operator_id, "PREPARE message for different root than accepted proposal");
            return;
        }

        // Check if we have reached a prepare quorum for this round, if so send the commit message
        if let Some(hash) = self.prepare_container.has_quorum(round) {
            // Make sure we are in the correct state
            let proposal_root = match self.state {
                InstanceState::Prepare { proposal_root } => proposal_root,
                _ => {
                    debug!(from=?operator_id, ?self.state, "Not in PREPARE state");
                    return;
                }
            };

            // Make sure that the root of the data that we have come to a prepare consensus on
            // matches the root of the proposal that we have accepted
            if hash != proposal_root {
                warn!("PREPARE quorum root does not match accepted PROPOSAL root");
                return;
            }

            // Success! We have come to a prepare consensus on a value

            // Move the state forward since we have a prepare quorum
            self.state = InstanceState::Commit { proposal_root };
            debug!(state = ?self.state, "Reached a PREPARE consensus. State updated to COMMIT");

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
        if u8::from(self.state) >= u8::from(InstanceState::SentRoundChange) {
            debug!(from=*operator_id, ?self.state, "COMMIT message while in invalid state");
            return;
        }

        // Make sure this is actually a commit message
        if !(matches!(
            wrapped_msg.qbft_message.qbft_message_type,
            QbftMessageType::Commit,
        )) {
            warn!(from=?operator_id, "Expected a COMMIT message");
            return;
        }

        // Handle commit message, checking proposal acceptance and catch-up scenarios.

        // If we have NOT accepted a proposal for this round, this is a catch-up scenario.
        // We allow commits without having seen a proposal in this case.
        if !self.proposal_accepted_for_current_round {
            debug!(from=?operator_id, ?self.state, "Have not accepted Proposal for current round yet (catch-up scenario)");
            return;
        }

        // Proposal accepted: ensure commit matches the accepted proposal root.
        if let Some(accepted_root) = self.proposal_root {
            if wrapped_msg.qbft_message.root != accepted_root {
                warn!(from=?operator_id, ?self.state, "Commit root does not match accepted Proposal root");
                return;
            }
        } else {
            warn!(from=?operator_id, ?self.state, "Proposal accepted, but no proposal root found");
            return;
        }

        debug!(from = ?operator_id, state = ?self.state, "COMMIT received");

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
            match self.state {
                InstanceState::Prepare { proposal_root }
                | InstanceState::Commit { proposal_root } => {
                    // We already accepted a proposal and are in commit state
                    if hash != proposal_root {
                        warn!("COMMIT quorum root does not match accepted PROPOSAL root");
                        return;
                    }
                }
                _ => return,
            }

            // Aggregate all of the commit messages
            let commit_quorum = self.commit_container.get_quorum_of_messages(round);
            let aggregated_commit = self.aggregate_commit_messages(commit_quorum);
            if aggregated_commit.is_some() {
                debug!(state = ?self.state, "Reached a COMMIT consensus. Success!");
                self.aggregated_commit = aggregated_commit;
                self.state = InstanceState::Complete;
                self.completed = Some(Completed::Success(hash));
            } else {
                error!("Failed to aggregate commit quorum")
            }
        }
    }

    // Aggregate a quorum of commit messages into one signed message
    fn aggregate_commit_messages(
        &self,
        commit_quorum: Vec<WrappedQbftMessage>,
    ) -> Option<SignedSSVMessage> {
        // We know this exists, but in favor of avoiding expect match the first element to Some.
        // This will be the commit message that we aggregate on top of
        if let Some(first_commit) = commit_quorum.first() {
            let mut aggregated_commit = first_commit.signed_message.clone();
            let aggregated_ssv = aggregated_commit.ssv_message();

            // Sanity check that all of the messages match
            commit_quorum[1..]
                .iter()
                .all(|commit_msg| aggregated_ssv == commit_msg.signed_message.ssv_message())
                .then_some(())?;

            // Aggregate all of the commits together
            let signed_commits = commit_quorum[1..]
                .iter()
                .map(|msg| msg.signed_message.clone());
            aggregated_commit.aggregate(signed_commits);

            // Set full data
            let hash = first_commit.qbft_message.root;
            aggregated_commit.set_full_data(self.data.get(&hash)?.as_ssz_bytes());

            return Some(aggregated_commit);
        }

        None
    }

    /// We have received a round change message.
    fn received_round_change(
        &mut self,
        operator_id: OperatorId,
        round: Round,
        wrapped_msg: WrappedQbftMessage,
    ) {
        // Make sure we are in the correct state
        if u8::from(self.state) >= u8::from(InstanceState::Complete) {
            debug!(from=*operator_id, ?self.state, "ROUNDCHANGE message while in invalid state");
            return;
        }

        let qbft_msg = &wrapped_msg.qbft_message;
        // If this is a "prepared" round change, we have to check the justifications.
        if qbft_msg.data_round > 0 {
            if !self.check_quorum(&qbft_msg.round_change_justification) {
                debug!(
                    from = *operator_id,
                    justifications = qbft_msg.round_change_justification.len(),
                    quorum = self.config.quorum_size(),
                    "prepared ROUNDCHANGE has no quorum"
                );
                return;
            }

            if qbft_msg.data_round > qbft_msg.round {
                debug!(
                    from = *operator_id,
                    data_round = qbft_msg.data_round,
                    round = qbft_msg.round,
                    "ROUNDCHANGE has prepared round after round"
                );
                return;
            }

            for justification in qbft_msg.round_change_justification.iter() {
                if !self.is_valid_prepare_justification_for_round_and_root(
                    justification,
                    qbft_msg.data_round.into(),
                    &qbft_msg.root,
                ) {
                    debug!(
                        from = *operator_id,
                        "ROUNDCHANGE has invalid prepare justification"
                    );
                    return;
                }
            }
        }

        debug!(from = ?operator_id, state = ?self.state, "ROUNDCHANGE received");

        // Check if we already have a quorum
        let had_quorum_before = self
            .round_change_container
            .has_quorum_disregarding_root(round);

        // Store the round changed message regardless
        if !self
            .round_change_container
            .add_message(round, operator_id, &wrapped_msg)
        {
            warn!(from = ?operator_id, "ROUNDCHANGE message is a duplicate")
        }

        // If we already had quorum, just return
        if had_quorum_before {
            debug!(from = ?operator_id, "Already had round change quorum, ignoring");
            return;
        }

        // There are two cases to check here

        // 1. If we have received a quorum of round change messages, we need to start a new round
        if self
            .round_change_container
            .has_quorum_disregarding_root(round)
        {
            debug!(round = *round, "Round change quorum reached");

            // We have reached consensus on a round change, we can start a new round now
            self.state = InstanceState::RoundChangeConsensus;

            // The round change messages is round + 1, so this is the next round we want to use
            self.set_round(round);
        } else {
            // 2. If we receive f+1 round change messages, we need to send our own round-change
            //    message
            let round = self
                .round_change_container
                .lowest_partial_quorum_above_round(self.current_round, self.config.get_f() + 1);
            if let Some(round) = round
                && round > self.current_round
            {
                self.state = InstanceState::SentRoundChange;
                self.current_round = round;
                self.proposal_accepted_for_current_round = false;
                self.send_round_change(Hash256::default());
            }
        }
    }

    // We have received a decided message
    fn received_decided(&mut self, wrapped_msg: WrappedQbftMessage) {
        // Make sure we have a quorum of signatures
        if wrapped_msg.signed_message.operator_ids().len() < self.config().quorum_size() {
            return;
        }

        // All message and signature verification has already succeeded. Regardless of what state
        // this instance is at, we have all of the information necessary to mark it as
        // complete
        self.state = InstanceState::Complete;
        self.completed = Some(Completed::Success(wrapped_msg.qbft_message.root));
        self.aggregated_commit = Some(wrapped_msg.signed_message);
    }

    // End the current round and move to the next one, if possible.
    pub fn end_round(&mut self) {
        debug!(round = *self.current_round, "Incrementing round");
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
                .map(|d| d.as_ssz_bytes())
                .unwrap_or_else(|| {
                    warn!("Proposal data missing for hash {:?}", data_hash);
                    vec![]
                })
        } else {
            vec![]
        };

        if matches!(msg_type, QbftMessageType::RoundChange) {
            if let (Some(last_prepared_value), Some(last_prepared_round)) =
                (self.last_prepared_value, self.last_prepared_round)
            {
                // When we have prepare justifications - use hash of last prepared value
                return MessageData::new(
                    last_prepared_round.get() as u64,
                    self.current_round.get() as u64,
                    last_prepared_value,
                    self.data
                        .get(&last_prepared_value)
                        .map(|d| d.as_ssz_bytes())
                        .unwrap_or_else(|| {
                            warn!("Data missing for last prepared value");
                            vec![]
                        }),
                );
            } else {
                // When we DON'T have prepare justifications - use empty root
                return MessageData::new(
                    0, // NoRound
                    self.current_round.get() as u64,
                    Hash256::default(),
                    vec![],
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
        mut round_change_justification: Vec<SignedSSVMessage>,
        mut prepare_justification: Vec<SignedSSVMessage>,
    ) -> UnsignedWrappedQbftMessage {
        let data = self.get_message_data(&msg_type, data_hash);

        // Clear full_data from justifications as these do not store full data.
        for round_change_justification in &mut round_change_justification {
            round_change_justification.set_full_data(vec![]);
        }
        for prepare_justification in &mut prepare_justification {
            prepare_justification.set_full_data(vec![]);
        }

        // Create the QBFT message
        let qbft_message = QbftMessage {
            qbft_message_type: msg_type,
            height: *self.instance_height as u64,
            round: data.round,
            identifier: (&self.identifier).into(),
            root: data.root,
            data_round: data.data_round,
            round_change_justification,
            prepare_justification,
        };

        let ssv_message = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            self.identifier.clone(),
            qbft_message.as_ssz_bytes(),
        )
        .expect("SSVMessage should be valid."); // TODO revisit this

        // Wrap in unsigned SSV message
        UnsignedWrappedQbftMessage {
            unsigned_message: UnsignedSSVMessage {
                ssv_message,
                full_data: data.full_data,
            },
            qbft_message,
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
            let round_changes = self
                .round_change_container
                .get_messages_for_round(self.current_round);

            // We need at least a quorum of round changes to justify the proposal
            if round_changes.len() >= self.config.quorum_size() {
                return round_changes
                    .into_iter()
                    .map(|msg| msg.signed_message.clone())
                    .collect();
            }
        }
        vec![]
    }

    /// Get justifications for a RoundChange message
    /// If we have prepared a value, include the Prepare messages that justify it
    fn get_round_change_prepare_justifications(&self) -> Vec<SignedSSVMessage> {
        // Only include prepare justifications if we have a prepared value
        if let (Some(last_prepared_value), Some(last_prepared_round)) =
            (self.last_prepared_value, self.last_prepared_round)
        {
            // Get the prepare messages for the round where we prepared
            let prepares = self
                .prepare_container
                .get_messages_for_round(last_prepared_round);

            // Only include prepares that match our prepared value
            let filtered_prepares: Vec<_> = prepares
                .iter()
                .filter(|msg| msg.qbft_message.root == last_prepared_value)
                .collect();

            // We need a quorum of prepares to justify the prepared value
            if filtered_prepares.len() >= self.config.quorum_size() {
                let result: Vec<SignedSSVMessage> = filtered_prepares
                    .into_iter()
                    .map(|msg| msg.signed_message.clone())
                    .collect();
                return result;
            }
        }

        vec![]
    }

    // Get all of the prepare justifications for proposals
    fn get_prepare_justifications(&self) -> (Vec<SignedSSVMessage>, Option<Hash256>) {
        // No justifications needed for round 1
        if self.current_round == Round::default() {
            return (vec![], None);
        }

        // Only needed when we're the proposer
        if !matches!(self.state, InstanceState::AwaitingProposal) {
            return (vec![], None);
        }

        // Check if we have our own prepared value that should be proposed
        // This handles the case where we prepared a value but the RoundChange messages
        // don't reflect it (e.g., other nodes didn't prepare)
        let potential_prepare_just = self.get_round_change_prepare_justifications();
        if !potential_prepare_just.is_empty() {
            if let Some(last_prepared) = self.last_prepared_value {
                return (potential_prepare_just, Some(last_prepared));
            } else {
                // Invariant violated: potential_prepare_just is not empty but no
                // last_prepared_value Handle gracefully: return no justification
                error!("prepare justifications exists but no last prepared value was found");
                return (vec![], None);
            }
        }

        // Get all round change messages for current round
        let round_changes = self
            .round_change_container
            .get_messages_for_round(self.current_round);

        if round_changes.len() < self.config.quorum_size() {
            return (vec![], None);
        }

        // Find the highest prepared round among all round changes
        let mut highest_prepared: Option<(Round, Hash256, &WrappedQbftMessage)> = None;

        for rc_msg in &round_changes {
            // Check if this round change has a prepared value
            if rc_msg.qbft_message.data_round > 0 {
                let prepared_round = Round::from(rc_msg.qbft_message.data_round);

                // Update if this is the highest we've seen
                if highest_prepared.is_none_or(|(round, _, _)| prepared_round > round) {
                    highest_prepared = Some((prepared_round, rc_msg.qbft_message.root, rc_msg));
                }
            }
        }

        // If we found a highest prepared value, extract its prepare justifications
        if let Some((_, prepared_value, highest_rc)) = highest_prepared {
            // Extract the prepare messages from the round change message's justifications
            // These are stored in the round_change_justification field of the RoundChange
            let prepares = &highest_rc.qbft_message.round_change_justification;

            // Verify we have quorum of prepares
            if prepares.len() >= self.config.quorum_size() {
                return (prepares.clone(), Some(prepared_value));
            }
        }

        // No prepared value found, proposer can choose new value
        (vec![], None)
    }

    // Send a new qbft proposal message
    fn send_proposal(&mut self, hash: D::Hash, data: Arc<D>) {
        // Store the data we're proposing
        self.data.insert(hash, data.clone());

        // For Proposal messages
        // round_change_justification: list of round change messages
        let round_change_justifications = self.get_round_change_justifications();
        // prepare_justification: list of prepare messages
        let (prepare_justifications, value_to_propose) = self.get_prepare_justifications();

        // Determine the value that should be proposed based off of justification. If we have a
        // prepare justification, we want to propose that value. Else, just the justified value
        let value_to_propose = value_to_propose.unwrap_or(hash);

        // Construct a unsigned proposal
        let unsigned_msg = self.new_unsigned_message(
            QbftMessageType::Proposal,
            value_to_propose,
            round_change_justifications,
            prepare_justifications,
        );

        self.message_sender.send(unsigned_msg);
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

        self.message_sender.send(unsigned_msg);
    }

    // Send a new qbft commit message
    fn send_commit(&mut self, data_hash: D::Hash) {
        // Construct unsigned commit
        let unsigned_msg =
            self.new_unsigned_message(QbftMessageType::Commit, data_hash, vec![], vec![]);

        self.message_sender.send(unsigned_msg);
    }

    // Send a new qbft round change message
    fn send_round_change(&mut self, data_hash: D::Hash) {
        // For Round Change messages
        // round_change_justification: list of prepare messages
        let round_change_justifications = self.get_round_change_prepare_justifications();
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

        self.message_sender.send(unsigned_msg);
    }

    /// Extract the data that the instance has come to consensus on
    pub fn completed(&self) -> Option<Completed<D>> {
        self.completed
            .clone()
            .and_then(|completed| match completed {
                // For timeout, we don't need any data
                Completed::TimedOut => Some(Completed::TimedOut),

                // For success, we need to find the actual data
                Completed::Success(hash) => {
                    // Try to get the Arc<D> from our data map
                    let data = self.data.get(&hash).cloned();

                    if data.is_none() {
                        error!("could not find finished data");
                    }

                    // Transform Arc<D> into Completed::Success(D)
                    data.map(|arc_data| Completed::Success((*arc_data).clone()))
                }
            })
    }
}
