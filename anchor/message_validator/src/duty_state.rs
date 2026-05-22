use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use ssv_types::{
    CommitteeId, Epoch, OperatorId, Slot,
    consensus::{QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
    partial_sig::PartialSignatureMessages,
};

use crate::{FIRST_ROUND, ValidationFailure, message_counts::MessageCounts};
// duty_state.rs
//
// This file defines structures that help track and validate the consensus process.
// The main components are:
//  - DutyState: The top-level state tracker across operators and slots.
//  - OperatorState: The state for a specific operator over a range of slots.
//  - SignerState: The state of a signer at a particular slot, including message counts and proposal
//    data.

/// DutyState manages the state for duty validation across operators and slots
pub(crate) struct DutyState {
    /// Tracks the duty state for an operator
    operators: HashMap<OperatorId, OperatorState>,
    /// The number of slots for which state is stored (defines the size of the circular buffer)
    stored_slot_count: usize,
}

impl DutyState {
    /// Creates a new DutyState with the specified storage capacity
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

    /// Updates the duty state with new incoming messages.
    ///
    /// For each operator involved in the signed message, this method:
    /// - Determines the corresponding slot and estimated epoch,
    /// - Retrieves or creates the operator's state,
    /// - And delegates the update to the operator's state.
    pub(crate) fn update_for_consensus_message(
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

    /// Updates the duty state with information about a partial signature message.
    /// This records the message type in the message counts for the signer at the given slot.
    pub(crate) fn update_for_partial_signature(
        &mut self,
        partial_signature_messages: &PartialSignatureMessages,
        signer: &OperatorId,
        slots_per_epoch: u64,
    ) -> Result<(), ValidationFailure> {
        let operator_state = self.get_or_create_operator(signer);
        let message_slot = partial_signature_messages.slot;
        let message_epoch = Epoch::new(message_slot.as_u64() / slots_per_epoch);

        // Get or create a signer state for this slot
        let signer_state = match operator_state.get_signer_state_mut(&message_slot) {
            Some(existing_state) => existing_state,
            _ => {
                // Create a new signer state
                let new_signer_state = SignerState::new(message_slot, FIRST_ROUND);
                operator_state.set_signer_state_for_first_round(
                    &message_slot,
                    &message_epoch,
                    new_signer_state,
                )
            }
        };

        // Record the partial signature (only once)
        signer_state
            .message_counts
            .record_partial_signature(partial_signature_messages.kind);

        Ok(())
    }

    /// Returns true if all operators within the map have a `max_slot` lower than `now -
    /// stored_slot_count`. This indicates that there has been no relevant activity for this duty
    /// recently and no relevant information is lost if this is dropped.
    pub(crate) fn outdated(&self, current_slot: Slot) -> bool {
        let earliest_relevant_slot =
            current_slot.saturating_sub(Slot::from(self.stored_slot_count));
        self.operators
            .values()
            .all(|operator_state| operator_state.max_slot < earliest_relevant_slot)
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
    curr_epoch_duties: u64,
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
            curr_epoch_duties: 0,
            prev_epoch_duties: 0,
        }
    }

    /// Retrieves the maximum slot number processed for this operator.
    pub(crate) fn max_slot(&self) -> Slot {
        self.max_slot
    }

    pub(crate) fn get_duty_count(&self, epoch: Epoch) -> u64 {
        match epoch {
            e if e == self.max_epoch => self.curr_epoch_duties,
            e if e == self.max_epoch - 1 => self.prev_epoch_duties,
            _ => 0, // unused because messages from too old epochs must be rejected in advance
        }
    }

    /// Retrieves a mutable SignerState reference for a given slot.
    pub(crate) fn get_signer_state_mut(&mut self, slot: &Slot) -> Option<&mut SignerState> {
        let len = self.state.len();
        self.state[slot.as_usize() % len]
            .as_mut()
            .filter(|s| s.slot == *slot)
    }

    /// Retrieves a SignerState reference for a given slot.
    pub(crate) fn get_signer_state(&self, slot: &Slot) -> Option<&SignerState> {
        let len = self.state.len();
        self.state[slot.as_usize() % len]
            .as_ref()
            .filter(|s| s.slot == *slot)
    }

    /// Sets the signer state for a round change in the circular buffer at the computed index.
    fn set_signer_state_for_round_change(
        &mut self,
        slot: &Slot,
        signer_state: SignerState,
    ) -> &mut SignerState {
        let index = slot.as_usize() % self.state.len();
        self.state[index] = Some(signer_state);
        self.state[index].as_mut().unwrap()
    }

    /// Returns true if we have not seen a message for a duty in `slot` yet.
    pub(crate) fn is_first_message_for_duty(&self, slot: Slot) -> bool {
        self.get_signer_state(&slot).is_none()
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
        let maybe_signer_state = self.get_signer_state_mut(msg_slot);

        let signer_state = if let Some(signer_state) = maybe_signer_state {
            if consensus_message.round > signer_state.round {
                let signer_state = SignerState::new(*msg_slot, consensus_message.round);
                self.set_signer_state_for_round_change(msg_slot, signer_state)
            } else {
                signer_state
            }
        } else {
            let signer_state = SignerState::new(*msg_slot, consensus_message.round);
            self.set_signer_state_for_first_round(msg_slot, estimated_msg_epoch, signer_state)
        };

        signer_state.update(signed_ssv_message, consensus_message);
    }

    /// Sets the SignerState for the first round of a slot and updates tracking for the maximum slot
    /// and epoch.
    ///
    /// - Inserts the signer state into the circular buffer.
    /// - Updates `max_slot` if the new slot is higher.
    /// - Updates `max_epoch` and resets duty counters if the epoch has advanced.
    fn set_signer_state_for_first_round(
        &mut self,
        msg_slot: &Slot,
        estimated_msg_epoch: &Epoch,
        signer_state: SignerState,
    ) -> &mut SignerState {
        let index = msg_slot.as_usize() % self.state.len();
        self.state[index] = Some(signer_state);

        if msg_slot > &self.max_slot {
            self.max_slot = *msg_slot;
        }

        match estimated_msg_epoch.cmp(&self.max_epoch) {
            Ordering::Greater => {
                self.max_epoch = *estimated_msg_epoch;
                self.prev_epoch_duties = self.curr_epoch_duties;
                self.curr_epoch_duties = 1;
            }
            Ordering::Equal => {
                self.curr_epoch_duties += 1;
            }
            Ordering::Less => {
                // Messages with epochs lower than the current max are aggregated into
                // previous epoch duties. It is assumed that such messages have already
                // been validated as not too outdated.
                self.prev_epoch_duties += 1;
            }
        }
        self.state[index].as_mut().unwrap()
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
    /// Holds the hash of the proposal data, if a proposal was received.
    pub(crate) proposal_hash: Option<[u8; 32]>,
    /// A set of CommitteeIds indicating which committees have already been seen.
    seen_signers: HashSet<CommitteeId>,
}

impl SignerState {
    /// Creates a new SignerState for a given slot and round.
    pub fn new(slot: Slot, round: u64) -> Self {
        Self {
            slot,
            round,
            message_counts: MessageCounts::default(),
            proposal_hash: None,
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
    /// - If the message is a proposal (and contains full data), it stores the hashed data.
    /// - If multiple operator IDs are present, it records the committee as seen.
    /// - Updates the message counts based on the message type.
    fn update(&mut self, signed_ssv_message: &SignedSSVMessage, consensus_message: &QbftMessage) {
        if !signed_ssv_message.full_data().is_empty()
            && consensus_message.qbft_message_type == QbftMessageType::Proposal
        {
            // We verified that the proposal data matches the root.
            self.proposal_hash = Some(*consensus_message.root);
        }

        if signed_ssv_message.operator_ids().len() > 1 {
            self.seen_signers
                .insert(signed_ssv_message.operator_ids().into());
        }

        self.message_counts.record_consensus_message(
            consensus_message.qbft_message_type,
            signed_ssv_message.operator_ids().len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use ssv_types::{OperatorId, Slot, consensus::QbftMessageType, msgid::Role};

    use super::*;
    use crate::{
        hash_data,
        tests::{QbftMessageBuilder, create_signed_consensus_message},
    };

    #[test]
    fn test_duty_state_update() {
        let mut duty_state = DutyState::new(10);

        let mut qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();

        let operator_id = OperatorId(1);

        let full_data = vec![1, 2, 3];
        *qbft_message.root = hash_data(&full_data);
        let signed_ssv_message = create_signed_consensus_message(
            qbft_message.clone(),
            vec![operator_id],
            full_data.clone(),
            vec![],
        );

        // Update the duty state
        duty_state.update_for_consensus_message(&signed_ssv_message, &qbft_message, 32);

        // Retrieve the operator state
        let operator_state = duty_state.get_or_create_operator(&operator_id);
        let slot = Slot::from(qbft_message.height);

        // Get the signer state for the slot
        if let Some(signer_state) = operator_state.get_signer_state(&slot) {
            // // Verify that the proposal data was correctly stored
            assert_eq!(
                signer_state.proposal_hash,
                Some(hash_data(&full_data)),
                "Proposal data should match the hashed full data"
            );

            // Verify message counts were updated
            assert_eq!(
                signer_state.message_counts.proposal, 1,
                "Message count for Proposal should be 1"
            );
        } else {
            panic!("SignerState should exist for the slot");
        }
    }

    #[test]
    fn test_decided_message_not_counted() {
        let mut duty_state = DutyState::new(10);

        // Create a commit message with a single signer (should be counted)
        let single_signer_commit =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Commit).build();

        let operator_id = OperatorId(1);

        let signed_single_signer = create_signed_consensus_message(
            single_signer_commit.clone(),
            vec![operator_id],
            vec![],
            vec![],
        );

        // Update duty state with single-signer commit
        duty_state.update_for_consensus_message(&signed_single_signer, &single_signer_commit, 32);

        // Create a commit message with multiple signers (decided message, should NOT be counted)
        let multi_signer_commit =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Commit).build();

        let signed_multi_signer = create_signed_consensus_message(
            multi_signer_commit.clone(),
            vec![OperatorId(1), OperatorId(2), OperatorId(3)],
            vec![],
            vec![],
        );

        // Update duty state with multi-signer commit
        duty_state.update_for_consensus_message(&signed_multi_signer, &multi_signer_commit, 32);

        // Retrieve the operator state
        let operator_state = duty_state.get_or_create_operator(&operator_id);
        let slot = Slot::from(single_signer_commit.height);

        // Get the signer state for the slot
        if let Some(signer_state) = operator_state.get_signer_state(&slot) {
            // Verify commit count is 1 (only the single-signer message was counted)
            assert_eq!(
                signer_state.message_counts.commit, 1,
                "Commit count should be 1 (only single-signer commit should be counted)"
            );
        } else {
            panic!("SignerState should exist for the slot");
        }
    }
}
