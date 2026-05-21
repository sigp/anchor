use ssv_types::{
    MAX_SIGNATURES,
    consensus::QbftMessageType,
    message::SignedSSVMessage,
    partial_sig::{PartialSignatureKind, PartialSignatureMessages},
};
use types::consts::altair::SYNC_COMMITTEE_SUBNET_COUNT;

use crate::ValidationFailure;

const MAX_MESSAGES_PER_ROUND: u8 = 1;
const MAX_CONTRIBUTION_PROOF_MESSAGES_PER_SLOT: u64 = SYNC_COMMITTEE_SUBNET_COUNT;
const MAX_CONTRIBUTION_PROOF_SIGNATURES_PER_SLOT: usize = MAX_SIGNATURES;

/// MessageCounts tracks different types of message counts per slot
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MessageCounts {
    pub(crate) pre_consensus: u8,
    pub(crate) contribution_proof_signatures: usize,
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
            | PartialSignatureKind::ValidatorRegistration
            | PartialSignatureKind::VoluntaryExit
            | PartialSignatureKind::AggregatorCommitteePartialSig => {
                if self.pre_consensus >= MAX_MESSAGES_PER_ROUND {
                    return Err(ValidationFailure::InvalidPartialSignatureTypeCount {
                        got: format!("pre-consensus, having {self:?}"),
                    });
                }
            }
            PartialSignatureKind::ContributionProofs => {
                let contribution_proof_signatures = self
                    .contribution_proof_signatures
                    .saturating_add(messages.messages.len());

                if u64::from(self.pre_consensus) >= MAX_CONTRIBUTION_PROOF_MESSAGES_PER_SLOT
                    || contribution_proof_signatures > MAX_CONTRIBUTION_PROOF_SIGNATURES_PER_SLOT
                {
                    return Err(ValidationFailure::InvalidPartialSignatureTypeCount {
                        got: format!("pre-consensus contribution proofs, having {self:?}"),
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
    pub fn record_partial_signature(&mut self, messages: &PartialSignatureMessages) {
        match messages.kind {
            PartialSignatureKind::RandaoPartialSig
            | PartialSignatureKind::SelectionProofPartialSig
            | PartialSignatureKind::ValidatorRegistration
            | PartialSignatureKind::VoluntaryExit
            | PartialSignatureKind::AggregatorCommitteePartialSig => self.pre_consensus += 1,
            PartialSignatureKind::ContributionProofs => {
                self.pre_consensus += 1;
                self.contribution_proof_signatures = self
                    .contribution_proof_signatures
                    .saturating_add(messages.messages.len());
            }
            PartialSignatureKind::PostConsensus => self.post_consensus += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use bls::Signature;
    use ssv_types::{
        OperatorId, Slot, ValidatorIndex, VariableList, partial_sig::PartialSignatureMessage,
    };
    use types::Hash256;

    use super::*;

    const TEST_SLOT: u64 = 0;

    fn messages(kind: PartialSignatureKind) -> PartialSignatureMessages {
        messages_with_count(kind, 0)
    }

    fn messages_with_count(
        kind: PartialSignatureKind,
        message_count: usize,
    ) -> PartialSignatureMessages {
        let partial_signature_message = PartialSignatureMessage {
            partial_signature: Signature::empty(),
            signing_root: Hash256::from([0u8; 32]),
            signer: OperatorId(1),
            validator_index: ValidatorIndex(0),
        };

        PartialSignatureMessages {
            kind,
            slot: Slot::new(TEST_SLOT),
            messages: VariableList::new(vec![partial_signature_message; message_count])
                .expect("test vector is within PartialSignatureMessages capacity"),
        }
    }

    fn contribution_proof_limit_as_count() -> u8 {
        MAX_CONTRIBUTION_PROOF_MESSAGES_PER_SLOT
            .try_into()
            .expect("sync committee subnet count fits in MessageCounts counter")
    }

    #[test]
    fn test_validate_partial_signature_message_allows_contribution_proofs_up_to_sync_subnet_count()
    {
        // Arrange
        let counts = MessageCounts {
            pre_consensus: contribution_proof_limit_as_count() - 1,
            ..Default::default()
        };
        let messages = messages(PartialSignatureKind::ContributionProofs);

        // Act
        let result = counts.validate_partial_signature_message(&messages);

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_partial_signature_message_rejects_contribution_proofs_over_sync_subnet_count()
    {
        // Arrange
        let counts = MessageCounts {
            pre_consensus: contribution_proof_limit_as_count(),
            ..Default::default()
        };
        let messages = messages(PartialSignatureKind::ContributionProofs);

        // Act
        let result = counts.validate_partial_signature_message(&messages);

        // Assert
        assert!(matches!(
            result,
            Err(ValidationFailure::InvalidPartialSignatureTypeCount { .. })
        ));
    }

    #[test]
    fn test_validate_partial_signature_message_rejects_contribution_proofs_over_signature_count() {
        // Arrange
        let counts = MessageCounts {
            pre_consensus: 1,
            contribution_proof_signatures: MAX_CONTRIBUTION_PROOF_SIGNATURES_PER_SLOT,
            ..Default::default()
        };
        let messages = messages_with_count(PartialSignatureKind::ContributionProofs, 1);

        // Act
        let result = counts.validate_partial_signature_message(&messages);

        // Assert
        assert!(matches!(
            result,
            Err(ValidationFailure::InvalidPartialSignatureTypeCount { .. })
        ));
    }

    #[test]
    fn test_record_partial_signature_tracks_contribution_proof_signature_count() {
        // Arrange
        let mut counts = MessageCounts::default();
        let messages = messages_with_count(PartialSignatureKind::ContributionProofs, 3);

        // Act
        counts.record_partial_signature(&messages);

        // Assert
        assert_eq!(counts.pre_consensus, 1);
        assert_eq!(counts.contribution_proof_signatures, 3);
    }

    #[test]
    fn test_validate_partial_signature_message_keeps_single_envelope_limit_for_selection_proofs() {
        // Arrange
        let counts = MessageCounts {
            pre_consensus: MAX_MESSAGES_PER_ROUND,
            ..Default::default()
        };
        let messages = messages(PartialSignatureKind::SelectionProofPartialSig);

        // Act
        let result = counts.validate_partial_signature_message(&messages);

        // Assert
        assert!(matches!(
            result,
            Err(ValidationFailure::InvalidPartialSignatureTypeCount { .. })
        ));
    }
}
