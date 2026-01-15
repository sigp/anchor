//! Deferred state updates for message validation.
//!
//! This module implements a pattern where validation checks are separated from state mutations.
//! Instead of updating state during validation, we return a `StateUpdate` describing what
//! changes should be made. The caller then decides when/if to apply these changes.
//!
//! This separation is critical for fork transitions where we may want to:
//! - Validate a message (for gossipsub propagation)
//! - But NOT update state (if we won't process the message)
//!
//! Without this separation, validating a message on one topic could prevent the same message
//! from being validated on another topic (due to duplicate detection state).

use ssv_types::{
    CommitteeId, Epoch, OperatorId, Slot, consensus::QbftMessageType,
    partial_sig::PartialSignatureKind,
};

/// Represents deferred state changes from validating a message.
///
/// This is returned alongside the validation result and should be applied
/// only when the message will actually be processed.
#[derive(Debug, Clone, Default)]
#[must_use = "StateUpdate should be committed via Validator::commit() when the message is processed"]
pub enum StateUpdate {
    /// No state update needed (e.g., message was rejected during validation)
    #[default]
    None,
    /// State update for a consensus (QBFT) message
    Consensus(ConsensusStateUpdate),
    /// State update for a partial signature message
    PartialSignature(PartialSignatureStateUpdate),
}

/// State update data for consensus messages.
#[derive(Debug, Clone)]
pub struct ConsensusStateUpdate {
    /// Operators who signed the message
    pub signers: Vec<OperatorId>,
    /// Slot the message is for
    pub slot: Slot,
    /// Estimated epoch (slot / slots_per_epoch)
    pub estimated_epoch: Epoch,
    /// Consensus round number
    pub round: u64,
    /// Type of consensus message
    pub message_type: QbftMessageType,
    /// Whether this is a multi-signer (decided) message
    pub is_multi_signer: bool,
    /// Hash of proposal data (if this is a proposal with full data)
    pub proposal_hash: Option<[u8; 32]>,
    /// Committee ID for multi-signer messages (for seen_signers tracking)
    pub committee_id: Option<CommitteeId>,
}

impl ConsensusStateUpdate {
    /// Create a new consensus state update.
    pub fn new(
        signers: Vec<OperatorId>,
        slot: Slot,
        estimated_epoch: Epoch,
        round: u64,
        message_type: QbftMessageType,
        proposal_hash: Option<[u8; 32]>,
    ) -> Self {
        let is_multi_signer = signers.len() > 1;
        let committee_id = if is_multi_signer {
            Some(signers.as_slice().into())
        } else {
            None
        };

        Self {
            signers,
            slot,
            estimated_epoch,
            round,
            message_type,
            is_multi_signer,
            proposal_hash,
            committee_id,
        }
    }
}

/// State update data for partial signature messages.
#[derive(Debug, Clone)]
pub struct PartialSignatureStateUpdate {
    /// The operator who sent the partial signature
    pub signer: OperatorId,
    /// Slot the message is for
    pub slot: Slot,
    /// Epoch of the message
    pub epoch: Epoch,
    /// Kind of partial signature
    pub kind: PartialSignatureKind,
}

impl PartialSignatureStateUpdate {
    /// Create a new partial signature state update.
    pub fn new(signer: OperatorId, slot: Slot, epoch: Epoch, kind: PartialSignatureKind) -> Self {
        Self {
            signer,
            slot,
            epoch,
            kind,
        }
    }
}
