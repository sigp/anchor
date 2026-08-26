use std::collections::{HashMap, HashSet};

use libp2p::PeerId;
use ssv_types::{
    CommitteeId, Epoch, OperatorId, Slot,
    consensus::{QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
    partial_sig::{PartialSignatureKind, PartialSignatureMessages},
};
use types::Hash256;

use crate::{FIRST_ROUND, ValidationFailure, message_counts::MessageCounts};
// duty_state.rs
//
// This file defines structures that help track and validate the consensus process.
// The main components are:
//  - DutyState: The top-level state tracker across operators and slots.
//  - OperatorState: The state for a specific operator over a range of slots.
//  - SignerState: The state of a signer at a particular slot, including message counts and proposal
//    data.

/// Maximum distinct `ProposerPreferences` signing roots accepted per
/// (`MessageId`, operator, `proposal_slot`).
///
/// A validator can legitimately sign several distinct roots for one `proposal_slot` when its
/// preference inputs change between emissions — chiefly a `dependent_root` shift under reorg (the
/// SIP-94 §5 re-emission trigger), and also `target_gas_limit` / `fee_recipient` config changes
/// across operator restarts. Per SIP-94 §7 the cap is policy headroom for a few realistic
/// reorg-driven corrections while bounding spam, not a safety/consensus bound. Both exceeding
/// the cap (`TooManyDistinctSigningRoots`) and repeating an already-recorded root
/// (`RelayedDuplicateMessage`) are Ignore, regardless of the propagation peer; rationale at the
/// enforcement site in `update_for_partial_signature`.
const MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS: usize = 4;

/// Test-only, crate-visible mirror of the private cap so pipeline tests in sibling modules
/// (e.g. `partial_signature`) can reference the real value instead of hardcoding `4`. The
/// compile-time assertion below makes the mirror impossible to drift from production.
#[cfg(test)]
pub(crate) const MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS_FOR_TEST: usize =
    MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS;
#[cfg(test)]
const _: () = assert!(
    MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS_FOR_TEST == MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS
);

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
    /// - Retrieves or creates the operator's state,
    /// - And delegates the update to the operator's state.
    pub(crate) fn update_for_consensus_message(
        &mut self,
        signed_ssv_message: &SignedSSVMessage,
        consensus_message: &QbftMessage,
    ) {
        let msg_slot = Slot::from(consensus_message.height);

        for signer in signed_ssv_message.operator_ids() {
            let operator_state = self.get_or_create_operator(signer);
            operator_state.update(signed_ssv_message, consensus_message, &msg_slot);
        }
    }

    /// Updates the duty state with information about a partial signature message.
    /// This records the message type in the message counts for the signer at the given slot.
    pub(crate) fn update_for_partial_signature(
        &mut self,
        partial_signature_messages: &PartialSignatureMessages,
        signer: &OperatorId,
        received_from: Option<PeerId>,
    ) -> Result<(), ValidationFailure> {
        let operator_state = self.get_or_create_operator(signer);
        let message_slot = partial_signature_messages.slot;

        // Get or create a signer state for this slot
        let signer_state = match operator_state.get_signer_state_mut(&message_slot) {
            Some(existing_state) => existing_state,
            _ => {
                // Create a new signer state
                let new_signer_state = SignerState::new(message_slot, FIRST_ROUND);
                operator_state.set_signer_state(&message_slot, new_signer_state)
            }
        };

        // ProposerPreferences-specific per-slot signing-root dedup (not the shared pre_consensus
        // counter): the envelope slot is the duty's proposal_slot, and a validator may sign several
        // distinct roots for one proposal_slot as its preference inputs change between emissions
        // (chiefly a dependent_root shift under reorg). Track the distinct roots per
        // (MessageId, operator, proposal_slot), capped at MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS.
        if partial_signature_messages.kind == PartialSignatureKind::ProposerPreferences {
            let root = partial_signature_messages
                .messages
                .first()
                .ok_or(ValidationFailure::NoPartialSignatureMessages)?
                .signing_root;
            // Any repeat of a recorded root is IGNORE regardless of the propagation peer
            // (SIP-94 §7): an honest retry or restart can repeat an accepted root after the
            // recipient's gossip duplicate cache expires, so repetition does not prove peer
            // fault. Membership is checked before capacity so a recorded root stays IGNORE
            // even when the set is full.
            if signer_state.seen_preferences.contains_key(&root) {
                return Err(ValidationFailure::RelayedDuplicateMessage {
                    got: format!("proposer-preferences root {root:?}"),
                });
            }
            if signer_state.seen_preferences.len() >= MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS {
                return Err(ValidationFailure::TooManyDistinctSigningRoots {
                    got: format!(
                        "proposer-preferences distinct roots exceed cap \
                         {MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS}"
                    ),
                });
            }
            signer_state.seen_preferences.insert(root, received_from);
        }

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
}

impl OperatorState {
    /// Initializes a new OperatorState with a circular buffer sized according to stored_slot_count.
    fn new(stored_slot_count: usize) -> Self {
        Self {
            state: vec![None; stored_slot_count],
            max_slot: Slot::new(0),
        }
    }

    /// Retrieves the maximum slot number processed for this operator.
    pub(crate) fn max_slot(&self) -> Slot {
        self.max_slot
    }

    /// Counts the duties recorded for `epoch` by probing the ring at each of the epoch's slots.
    ///
    /// The ring holds exactly one occupied entry per counted duty slot, so occupancy is an
    /// exact per-epoch count (SIP-94 §7 retention) as long as the ring spans the role's whole
    /// message acceptance window; `stored_slot_count` owns that sizing guarantee.
    pub(crate) fn get_duty_count(&self, epoch: Epoch, slots_per_epoch: u64) -> u64 {
        epoch
            .slot_iter(slots_per_epoch)
            .filter(|slot| self.get_signer_state(slot).is_some())
            .count() as u64
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
    ) {
        let maybe_signer_state = self.get_signer_state_mut(msg_slot);

        let signer_state = if let Some(signer_state) = maybe_signer_state {
            if consensus_message.round > signer_state.round {
                let signer_state = SignerState::new(*msg_slot, consensus_message.round);
                self.set_signer_state(msg_slot, signer_state)
            } else {
                signer_state
            }
        } else {
            let signer_state = SignerState::new(*msg_slot, consensus_message.round);
            self.set_signer_state(msg_slot, signer_state)
        };

        signer_state.update(signed_ssv_message, consensus_message);
    }

    /// Sets the SignerState for a slot (first message or round change alike) and updates
    /// tracking for the maximum slot.
    ///
    /// - Inserts the signer state into the circular buffer.
    /// - Updates `max_slot` if the new slot is higher.
    fn set_signer_state(&mut self, msg_slot: &Slot, signer_state: SignerState) -> &mut SignerState {
        let index = msg_slot.as_usize() % self.state.len();
        self.state[index] = Some(signer_state);

        if msg_slot > &self.max_slot {
            self.max_slot = *msg_slot;
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
    /// Accepted ProposerPreferences signing roots for this (MessageId, operator, slot), each
    /// mapped to its first deliverer (`None` = locally injected). The verdict for a repeat is
    /// peer-agnostic Ignore (SIP-94 §7); the stored deliverer no longer affects classification
    /// and is retained only because issue #1254 scopes the peer plumbing as unchanged.
    seen_preferences: HashMap<Hash256, Option<PeerId>>,
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
            seen_preferences: HashMap::new(),
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
        duty_state.update_for_consensus_message(&signed_ssv_message, &qbft_message);

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
        duty_state.update_for_consensus_message(&signed_single_signer, &single_signer_commit);

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
        duty_state.update_for_consensus_message(&signed_multi_signer, &multi_signer_commit);

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

    /// Slots per epoch used by the ring-derived duty-count tests.
    const SLOTS_PER_EPOCH: u64 = 32;

    /// Role-8 (`ProposerPreferences`) ring size via the production selector, so these tests
    /// cannot drift from the sizing `get_duty_count`'s exactness depends on. Mainnet-shaped
    /// params give `(1 + min_seed_lookahead) * spe + 2 * spe = 128`, four concurrent epochs.
    fn role_8_ring() -> usize {
        crate::stored_slot_count(
            Role::ProposerPreferences,
            SLOTS_PER_EPOCH,
            &crate::tests::spec_with_gloas(None),
        )
    }

    /// Records a duty for `operator_id` at `slot` by feeding a consensus message through the
    /// production update path.
    fn record_duty_at_slot(duty_state: &mut DutyState, operator_id: OperatorId, slot: Slot) {
        let qbft_message =
            QbftMessageBuilder::new(Role::ProposerPreferences, QbftMessageType::Proposal)
                .with_height(slot.as_u64())
                .build();
        let signed_ssv_message = create_signed_consensus_message(
            qbft_message.clone(),
            vec![operator_id],
            vec![],
            vec![],
        );
        duty_state.update_for_consensus_message(&signed_ssv_message, &qbft_message);
    }

    #[test]
    fn test_duty_counts_survive_later_epoch_acceptance() {
        // Ring-derived counts must stay exact for EVERY epoch still inside the acceptance
        // window, not just the newest one. The deleted two-bucket counters pinned only
        // (max_epoch, max_epoch - 1): accepting an E+2 message advanced max_epoch to E+2,
        // relabeling E's count as E+1's and zeroing reads for E.
        let mut duty_state = DutyState::new(role_8_ring());
        let operator_id = OperatorId(1);
        let epoch = Epoch::new(10); // slots 320..=351

        // 3 duties in E, 2 in E+1, then 1 in E+2 (the later-epoch acceptance).
        let epoch_start = epoch.start_slot(SLOTS_PER_EPOCH);
        for offset in [0, 7, 31] {
            record_duty_at_slot(&mut duty_state, operator_id, epoch_start + offset);
        }
        let next_epoch_start = (epoch + 1).start_slot(SLOTS_PER_EPOCH);
        for offset in [3, 20] {
            record_duty_at_slot(&mut duty_state, operator_id, next_epoch_start + offset);
        }
        record_duty_at_slot(
            &mut duty_state,
            operator_id,
            (epoch + 2).start_slot(SLOTS_PER_EPOCH) + 5,
        );

        let operator_state = duty_state.get_or_create_operator(&operator_id);

        // E's count survives the E+2 acceptance (the old bucket code returned 0 for E here).
        assert_eq!(
            operator_state.get_duty_count(epoch, SLOTS_PER_EPOCH),
            3,
            "epoch E count must survive acceptance of a later-epoch message"
        );
        assert_eq!(
            operator_state.get_duty_count(epoch + 1, SLOTS_PER_EPOCH),
            2,
            "epoch E+1 count should be exact"
        );
        assert_eq!(
            operator_state.get_duty_count(epoch + 2, SLOTS_PER_EPOCH),
            1,
            "epoch E+2 count should be exact"
        );
        assert_eq!(
            operator_state.get_duty_count(epoch - 1, SLOTS_PER_EPOCH),
            0,
            "epoch E-1 has no recorded duties"
        );
        assert_eq!(
            operator_state.get_duty_count(epoch + 3, SLOTS_PER_EPOCH),
            0,
            "epoch E+3 has no recorded duties"
        );
    }

    #[test]
    fn test_duty_counts_not_conflated_across_older_epochs() {
        // Duties arriving for epochs older than the newest-seen one must be attributed to
        // their own epochs. The deleted counters' Ordering::Less arm lumped every epoch below
        // max_epoch into the single prev bucket: with max_epoch at E+2, the 2 duties in E and
        // 3 in E+1 below would all read as 5 for E+1 and 0 for E.
        let mut duty_state = DutyState::new(role_8_ring());
        let operator_id = OperatorId(1);
        let epoch = Epoch::new(10);

        // 1 duty in E+2 first, so the newest-seen epoch sits two ahead of the others.
        record_duty_at_slot(
            &mut duty_state,
            operator_id,
            (epoch + 2).start_slot(SLOTS_PER_EPOCH) + 5,
        );
        // Then 2 duties in E and 3 in E+1.
        let epoch_start = epoch.start_slot(SLOTS_PER_EPOCH);
        for offset in [1, 30] {
            record_duty_at_slot(&mut duty_state, operator_id, epoch_start + offset);
        }
        let next_epoch_start = (epoch + 1).start_slot(SLOTS_PER_EPOCH);
        for offset in [0, 11, 31] {
            record_duty_at_slot(&mut duty_state, operator_id, next_epoch_start + offset);
        }

        let operator_state = duty_state.get_or_create_operator(&operator_id);

        assert_eq!(
            operator_state.get_duty_count(epoch, SLOTS_PER_EPOCH),
            2,
            "epoch E count must not be conflated into a shared older-epoch bucket"
        );
        assert_eq!(
            operator_state.get_duty_count(epoch + 1, SLOTS_PER_EPOCH),
            3,
            "epoch E+1 count must not absorb epoch E's duties"
        );
        assert_eq!(
            operator_state.get_duty_count(epoch + 2, SLOTS_PER_EPOCH),
            1,
            "epoch E+2 count should be exact"
        );
    }

    #[test]
    fn test_duty_counts_ignore_aliased_ring_entries() {
        // The ring is modulo-indexed, so slots exactly `ring size` apart share an index. An
        // occupied index only counts toward the epoch of the slot actually stored there:
        // probing must check the stored slot, not raw index occupancy, or a stale entry left
        // behind by an evicted epoch would be counted into a live epoch.
        let mut duty_state = DutyState::new(role_8_ring());
        let operator_id = OperatorId(1);

        // One duty at slot 325 (epoch 10, offset 5), ring index 325 % 128 = 69.
        record_duty_at_slot(
            &mut duty_state,
            operator_id,
            Epoch::new(10).start_slot(SLOTS_PER_EPOCH) + 5,
        );

        let operator_state = duty_state.get_or_create_operator(&operator_id);

        // Epoch 14 spans slots 448..=479, whose ring indices 64..=95 include index 69 (via
        // slot 453). The entry stored there belongs to slot 325, so it must not be counted.
        assert_eq!(
            operator_state.get_duty_count(Epoch::new(14), SLOTS_PER_EPOCH),
            0,
            "an aliased ring entry from another epoch's slot must not be counted"
        );
        // Self-check: the same entry still counts toward its own epoch.
        assert_eq!(
            operator_state.get_duty_count(Epoch::new(10), SLOTS_PER_EPOCH),
            1,
            "the recorded duty must count toward the epoch of its stored slot"
        );
    }
}
