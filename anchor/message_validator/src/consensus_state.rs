use crate::message_counts::MessageCounts;
use ssv_types::consensus::{QbftMessage, QbftMessageType};
use ssv_types::message::SignedSSVMessage;
use ssv_types::OperatorId;
use ssv_types::{Epoch, Slot};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// ConsensusState manages the state for consensus validation across operators and slots
pub(crate) struct ConsensusState {
    operators: HashMap<OperatorId, RefCell<OperatorState>>,
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

    /// Gets or creates an operator state for the given signer
    pub(crate) fn get_or_create_operator(&mut self, signer: &OperatorId) -> RefCell<OperatorState> {
        self.operators
            .entry(*signer)
            .or_insert_with(|| RefCell::new(OperatorState::new(self.stored_slot_count)))
            .clone()
    }

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
            operator_state.borrow_mut().update(
                signed_ssv_message,
                consensus_message,
                &msg_slot,
                &estimated_msg_epoch,
            );
        }
    }
}

/// Tracks state for a specific operator across slots
#[derive(Clone)]
pub struct OperatorState {
    state: Vec<Option<SignerState>>,
    max_slot: Slot,
    max_epoch: Epoch,
    last_epoch_duties: u64,
    prev_epoch_duties: u64,
}

impl OperatorState {
    fn new(stored_slot_count: usize) -> Self {
        Self {
            state: vec![None; stored_slot_count],
            max_slot: Slot::new(0),
            max_epoch: Epoch::new(0),
            last_epoch_duties: 0,
            prev_epoch_duties: 0,
        }
    }

    pub(crate) fn get_signer_state(&self, slot: &Slot) -> Option<SignerState> {
        match &self.state[slot.as_usize() % self.state.len()] {
            Some(s) if s.slot == *slot => Some(s.clone()),
            _ => None,
        }
    }

    fn set_signer_state(&mut self, slot: &Slot, signer_state: &SignerState) {
        let index = slot.as_usize() % self.state.len();
        self.state[index] = Some(signer_state.clone());
    }

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

/// SignerState represents the state of a signer for a specific slot
#[derive(Debug, Clone)]
pub(crate) struct SignerState {
    slot: Slot,
    pub(crate) round: u64,
    pub(crate) message_counts: MessageCounts,
    pub(crate) proposal_data: Option<Vec<u8>>,
    seen_signers: HashSet<[u8; 32]>, // Changed key type to [u8; 32] for SHA-256 hash
}

impl SignerState {
    fn new(slot: Slot, round: u64) -> Self {
        Self {
            slot,
            round,
            message_counts: MessageCounts::default(),
            proposal_data: None,
            seen_signers: HashSet::new(),
        }
    }

    /// Checks if we've seen signers with this hash before
    pub(crate) fn has_seen_signers(&self, operators: &Vec<OperatorId>) -> bool {
        self.seen_signers.contains(&hash_operators(operators))
    }

    fn update(&mut self, signed_ssv_message: &SignedSSVMessage, consensus_message: &QbftMessage) {
        if !signed_ssv_message.full_data().is_empty()
            && consensus_message.qbft_message_type == QbftMessageType::Proposal
        {
            self.proposal_data = Some(Vec::from(signed_ssv_message.full_data()));
        }

        if signed_ssv_message.operator_ids().len() > 1 {
            self.seen_signers
                .insert(hash_operators(signed_ssv_message.operator_ids()));
        }

        self.message_counts
            .record_consensus_message(consensus_message.qbft_message_type);
    }
}

/// Hash a list of operator IDs to create a unique identifier, using SHA-256
fn hash_operators(operators: &Vec<OperatorId>) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    // Create a buffer (no sorting, to match Go implementation)
    let mut buffer = Vec::new();

    // Write each operator ID to the buffer in little endian order
    for &operator in operators {
        buffer.extend_from_slice(&operator.0.to_le_bytes());
    }

    // Hash the buffer with SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&buffer);
    hasher.finalize().into()
}
