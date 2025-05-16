use std::fmt;

use ssv_types::{consensus::QbftMessageType, message::SignedSSVMessage};

use crate::ValidationFailure;

const MAX_MESSAGES_PER_ROUND: u64 = 1;

/// MessageCounts tracks different types of message counts per slot
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MessageCounts {
    pub(crate) proposal: u64,
    pub(crate) prepare: u64,
    pub(crate) commit: u64,
    pub(crate) round_change: u64,
}

impl fmt::Display for MessageCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MessageCounts {{ proposal: {}, prepare: {}, commit: {}, round_change: {} }}",
            self.proposal, self.prepare, self.commit, self.round_change
        )
    }
}

impl MessageCounts {
    /// Validates if the message type exceeds the allowed limits
    pub fn validate_limits(
        &self,
        signed_message: &SignedSSVMessage,
        msg_type: QbftMessageType,
    ) -> Result<(), ValidationFailure> {
        match msg_type {
            QbftMessageType::Proposal if self.proposal >= MAX_MESSAGES_PER_ROUND => {
                Err(ValidationFailure::DuplicatedMessage {
                    got: format!("proposal, having {self}"),
                })
            }
            QbftMessageType::Prepare if self.prepare >= MAX_MESSAGES_PER_ROUND => {
                Err(ValidationFailure::DuplicatedMessage {
                    got: format!("prepare, having {self}"),
                })
            }
            QbftMessageType::Commit
                if signed_message.operator_ids().len() == 1
                    && self.commit >= MAX_MESSAGES_PER_ROUND =>
            {
                Err(ValidationFailure::DuplicatedMessage {
                    got: format!("commit, having {self}"),
                })
            }
            QbftMessageType::RoundChange if self.round_change >= MAX_MESSAGES_PER_ROUND => {
                Err(ValidationFailure::DuplicatedMessage {
                    got: format!("round change, having {self}"),
                })
            }
            _ => Ok(()),
        }
    }

    pub fn record_consensus_message(&mut self, msg_type: QbftMessageType) {
        // Increment the appropriate message counter
        match msg_type {
            QbftMessageType::Proposal => self.proposal += 1,
            QbftMessageType::Prepare => self.prepare += 1,
            QbftMessageType::Commit => self.commit += 1,
            QbftMessageType::RoundChange => self.round_change += 1,
        }
    }
}
