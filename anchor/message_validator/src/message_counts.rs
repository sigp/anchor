use ssv_types::{
    consensus::QbftMessageType,
    message::SignedSSVMessage,
    partial_sig::{PartialSignatureKind, PartialSignatureMessages},
};

use crate::ValidationFailure;

const MAX_MESSAGES_PER_ROUND: u8 = 1;

/// MessageCounts tracks different types of message counts per slot
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MessageCounts {
    pub(crate) pre_consensus: u8,
    pub(crate) proposal: u8,
    pub(crate) prepare: u8,
    pub(crate) commit: u8,
    pub(crate) round_change: u8,
    pub(crate) post_consensus: u8,
}

impl MessageCounts {
    /// Validates if the message type exceeds the allowed limits
    pub fn validate_consensus_message_limits(
        &self,
        signed_message: &SignedSSVMessage,
        msg_type: QbftMessageType,
    ) -> Result<(), ValidationFailure> {
        match msg_type {
            QbftMessageType::Proposal if self.proposal >= MAX_MESSAGES_PER_ROUND => {
                Err(ValidationFailure::DuplicatedMessage {
                    got: format!("proposal, having {self:?}"),
                })
            }
            QbftMessageType::Prepare if self.prepare >= MAX_MESSAGES_PER_ROUND => {
                Err(ValidationFailure::DuplicatedMessage {
                    got: format!("prepare, having {self:?}"),
                })
            }
            QbftMessageType::Commit
                if signed_message.operator_ids().len() == 1
                    && self.commit >= MAX_MESSAGES_PER_ROUND =>
            {
                Err(ValidationFailure::DuplicatedMessage {
                    got: format!("commit, having {self:?}"),
                })
            }
            QbftMessageType::RoundChange if self.round_change >= MAX_MESSAGES_PER_ROUND => {
                Err(ValidationFailure::DuplicatedMessage {
                    got: format!("round change, having {self:?}"),
                })
            }
            _ => Ok(()),
        }
    }

    /// Validates if the provided partial signature message exceeds the set limits.
    /// Returns an error if the message type exceeds its respective count limit.
    pub fn validate_partial_signature_message(
        &self,
        messages: &PartialSignatureMessages,
    ) -> Result<(), ValidationFailure> {
        match messages.kind {
            PartialSignatureKind::RandaoPartialSig
            | PartialSignatureKind::SelectionProofPartialSig
            | PartialSignatureKind::ContributionProofs
            | PartialSignatureKind::ValidatorRegistration
            | PartialSignatureKind::VoluntaryExit => {
                if self.pre_consensus >= MAX_MESSAGES_PER_ROUND {
                    return Err(ValidationFailure::InvalidPartialSignatureTypeCount {
                        got: format!("pre-consensus, having {self:?}"),
                    });
                }
            }
            PartialSignatureKind::PostConsensus => {
                if self.post_consensus >= MAX_MESSAGES_PER_ROUND {
                    return Err(ValidationFailure::InvalidPartialSignatureTypeCount {
                        got: format!("post-consensus, having {self:?}"),
                    });
                }
            }
        }

        Ok(())
    }

    pub fn record_consensus_message(&mut self, msg_type: QbftMessageType, signer_count: usize) {
        // Increment the appropriate message counter
        match msg_type {
            QbftMessageType::Proposal => self.proposal += 1,
            QbftMessageType::Prepare => self.prepare += 1,
            QbftMessageType::Commit => {
                // Commit messages with more than one signer (also known as Decided messages) are
                // not counted
                if signer_count == 1 {
                    self.commit += 1;
                }
            }
            QbftMessageType::RoundChange => self.round_change += 1,
        }
    }

    /// Records a partial signature message by incrementing the appropriate counter
    pub fn record_partial_signature(&mut self, kind: PartialSignatureKind) {
        match kind {
            PartialSignatureKind::RandaoPartialSig
            | PartialSignatureKind::SelectionProofPartialSig
            | PartialSignatureKind::ContributionProofs
            | PartialSignatureKind::ValidatorRegistration
            | PartialSignatureKind::VoluntaryExit => self.pre_consensus += 1,
            PartialSignatureKind::PostConsensus => self.post_consensus += 1,
        }
    }
}
