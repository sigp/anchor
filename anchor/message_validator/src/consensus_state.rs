use std::collections::{HashMap, HashSet};

use ssv_types::{
    consensus::{QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
    CommitteeId, Epoch, OperatorId, Slot,
};

use crate::message_counts::MessageCounts;

// consensus_state.rs
//
// This file defines structures that help track and validate the consensus process.
// The main components are:
//  - ConsensusState: The top-level state tracker across operators and slots.
//  - OperatorState: The state for a specific operator over a range of slots.
//  - SignerState: The state of a signer at a particular slot, including message counts and proposal
//    data.

/// ConsensusState manages the state for consensus validation across operators and slots
pub(crate) struct ConsensusState {
    /// Tracks the consensus state for an operator
    operators: HashMap<OperatorId, OperatorState>,
    /// The number of slots for which state is stored (defines the size of the circular buffer)
    stored_slot_count: usize,
}

impl ConsensusState {
    /// Creates a new ConsensusState with the specified storage capacity
    pub(crate) fn new(stored_slot_count: usize) -> Self {
        Self {
            operators: HashMap::new(),
            stored_slot_count,
        }
    }

    /// Retrieves an existing OperatorState for the given signer or creates one if it doesn't exist.
    /// This ensures that every operator has an associated state tracking its consensus messages.
    pub(crate) fn get_or_create_operator(&mut self, signer: &OperatorId) -> &mut OperatorState {
        self.operators
            .entry(*signer)
            .or_insert_with(|| OperatorState::new(self.stored_slot_count))
    }

    /// Updates the consensus state with new incoming messages.
    ///
    /// For each operator involved in the signed message, this method:
    /// - Determines the corresponding slot and estimated epoch,
    /// - Retrieves or creates the operator's state,
    /// - And delegates the update to the operator's state.
    pub fn update(
        &mut self,
        signed_ssv_message: &SignedSSVMessage,
        consensus_message: &QbftMessage,
        slots_per_epoch: u64,
    ) {
        let msg_slot = Slot::from(consensus_message.height);
        let estimated_msg_epoch = Epoch::new(msg_slot.as_u64() / slots_per_epoch);

        for signer in signed_ssv_message.operator_ids() {
            let operator_state = self.get_or_create_operator(signer);
            operator_state.update(
                signed_ssv_message,
                consensus_message,
                &msg_slot,
                &estimated_msg_epoch,
            );
        }
    }
}

/// Tracks the state for a specific operator across multiple slots.
///
/// This structure uses a fixed-size vector as a circular buffer to store the state
/// (SignerState) for different slots.
#[derive(Clone)]
pub struct OperatorState {
    /// A circular buffer (vector) where each index holds an Option<SignerState> for a slot.
    state: Vec<Option<SignerState>>,
    /// The highest slot number that has been processed for this operator.
    max_slot: Slot,
    /// The highest epoch number that has been processed.
    max_epoch: Epoch,
    /// The count of duties processed in the current epoch.
    last_epoch_duties: u64,
    /// The count of duties processed in the previous epoch.
    prev_epoch_duties: u64,
}

impl OperatorState {
    /// Initializes a new OperatorState with a circular buffer sized according to stored_slot_count.
    fn new(stored_slot_count: usize) -> Self {
        Self {
            state: vec![None; stored_slot_count],
            max_slot: Slot::new(0),
            max_epoch: Epoch::new(0),
            last_epoch_duties: 0,
            prev_epoch_duties: 0,
        }
    }

    /// Retrieves the SignerState for a given slot, if it exists.
    ///
    /// The state is stored in a circular buffer and is accessed using modulo arithmetic.
    pub(crate) fn get_signer_state(&self, slot: &Slot) -> Option<SignerState> {
        match &self.state[slot.as_usize() % self.state.len()] {
            Some(s) if s.slot == *slot => Some(s.clone()),
            _ => None,
        }
    }

    /// Sets the signer state in the circular buffer at the computed index.
    fn set_signer_state(&mut self, slot: &Slot, signer_state: &SignerState) {
        let index = slot.as_usize() % self.state.len();
        self.state[index] = Some(signer_state.clone());
    }

    /// Updates the SignerState for the given slot.
    ///
    /// If a state already exists and the incoming consensus round is higher,
    /// it replaces the state with a new one. Otherwise, it creates a new state
    /// if none exists for that slot.
    fn update(
        &mut self,
        signed_ssv_message: &SignedSSVMessage,
        consensus_message: &QbftMessage,
        msg_slot: &Slot,
        estimated_msg_epoch: &Epoch,
    ) {
        let maybe_signer_state = self.get_signer_state(msg_slot);

        let mut signer_state = if let Some(signer_state) = maybe_signer_state {
            if consensus_message.round > signer_state.round {
                let signer_state = SignerState::new(*msg_slot, consensus_message.round);
                self.set_signer_state(msg_slot, &signer_state);
                signer_state
            } else {
                signer_state
            }
        } else {
            let signer_state = SignerState::new(*msg_slot, consensus_message.round);
            self.set(msg_slot, estimated_msg_epoch, &signer_state);
            signer_state
        };

        signer_state.update(signed_ssv_message, consensus_message);
    }

    /// Sets the SignerState for a slot and updates tracking for the maximum slot and epoch.
    ///
    /// - Inserts the signer state into the circular buffer.
    /// - Updates `max_slot` if the new slot is higher.
    /// - Updates `max_epoch` and resets duty counters if the epoch has advanced.
    fn set(&mut self, msg_slot: &Slot, estimated_msg_epoch: &Epoch, signer_state: &SignerState) {
        let index = msg_slot.as_usize() % self.state.len();
        self.state[index] = Some(signer_state.clone());

        if msg_slot > &self.max_slot {
            self.max_slot = *msg_slot;
        }

        if estimated_msg_epoch > &self.max_epoch {
            self.max_epoch = *estimated_msg_epoch;
            self.prev_epoch_duties = self.last_epoch_duties;
            self.last_epoch_duties = 1;
        } else {
            self.last_epoch_duties += 1;
        }
    }
}

/// SignerState represents the state of a signer for a specific slot.
///
/// This structure tracks details of consensus processing for a given slot,
/// including the consensus round, counts of messages received, any proposal data,
/// and which committee signers have been observed to prevent duplicate processing.
#[derive(Debug, Clone)]
pub(crate) struct SignerState {
    /// The specific slot for which this state is maintained.
    slot: Slot,
    /// The consensus round number associated with this slot.
    pub(crate) round: u64,
    /// Records the count of each type of consensus message encountered.
    pub(crate) message_counts: MessageCounts,
    /// Optionally holds proposal-related data if a proposal message was received.
    pub(crate) proposal_data: Option<Vec<u8>>,
    /// A set of CommitteeIds indicating which committees have already been seen.
    seen_signers: HashSet<CommitteeId>,
}

impl SignerState {
    /// Creates a new SignerState for a given slot and round.
    fn new(slot: Slot, round: u64) -> Self {
        Self {
            slot,
            round,
            message_counts: MessageCounts::default(),
            proposal_data: None,
            seen_signers: HashSet::new(),
        }
    }

    /// Checks whether the signers (as represented by operator IDs) have been seen before.
    ///
    /// This helps prevent processing duplicate messages from the same committee.
    pub(crate) fn has_seen_signers(&self, operators: &[OperatorId]) -> bool {
        self.seen_signers.contains(&operators.into())
    }

    /// Updates the SignerState with a new consensus message.
    ///
    /// - If the message is a proposal (and contains full data), it stores the proposal data.
    /// - If multiple operator IDs are present, it records the committee as seen.
    /// - Updates the message counts based on the message type.
    fn update(&mut self, signed_ssv_message: &SignedSSVMessage, consensus_message: &QbftMessage) {
        if !signed_ssv_message.full_data().is_empty()
            && consensus_message.qbft_message_type == QbftMessageType::Proposal
        {
            self.proposal_data = Some(Vec::from(signed_ssv_message.full_data()));
        }

        if signed_ssv_message.operator_ids().len() > 1 {
            self.seen_signers
                .insert(signed_ssv_message.operator_ids().as_slice().into());
        }

        self.message_counts
            .record_consensus_message(consensus_message.qbft_message_type);
    }
}
