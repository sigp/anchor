mod consensus_message;
mod consensus_state;
mod message_counts;
mod partial_signature;

use std::{sync::Arc, time::SystemTime};

use dashmap::DashMap;
use database::NetworkState;
use gossipsub::MessageAcceptance;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use slot_clock::SlotClock;
use ssv_types::{
    consensus::QbftMessage,
    message::{MsgType, SignedSSVMessage},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::PartialSignatureMessages,
    CommitteeInfo, OperatorId,
};
use ssz::Decode;
use tokio::sync::watch::Receiver;
use tracing::{error, trace};

use crate::{
    consensus_message::validate_consensus_message, consensus_state::ConsensusState,
    partial_signature::validate_partial_signature_message,
};

// TODO taken from go-SSV as rough guidance. feel free to adjust as needed. https://github.com/ssvlabs/ssv/blob/e12abf7dfbbd068b99612fa2ebbe7e3372e57280/message/validation/errors.go#L55
#[derive(Debug)]
pub enum ValidationFailure {
    WrongDomain,
    NoShareMetadata,
    UnknownValidator,
    ValidatorLiquidated,
    ValidatorNotAttesting,
    EarlySlotMessage,
    LateSlotMessage,
    SlotAlreadyAdvanced,
    RoundAlreadyAdvanced {
        got: u64,
        want: u64,
    },
    DecidedWithSameSigners,
    PubSubDataTooBig(usize),
    IncorrectTopic,
    NonExistentCommitteeID,
    RoundTooHigh,
    ValidatorIndexMismatch,
    TooManyDutiesPerEpoch,
    NoDuty,
    EstimatedRoundNotInAllowedSpread {
        got: String,
        want: String,
    },
    EmptyData,
    MismatchedIdentifier {
        got: String,
        want: String,
    },
    SignatureVerification,
    PubSubMessageHasNoData,
    MalformedPubSubMessage,
    NilSignedSSVMessage,
    NilSSVMessage,
    SSVDataTooBig,
    InvalidRole,
    UnexpectedConsensusMessage,
    NoSigners,
    WrongRSASignatureSize,
    ZeroSigner,
    SignerNotInCommittee,
    DuplicatedSigner,
    SignerNotLeader {
        signer: OperatorId,
        leader: OperatorId,
    },
    SignersNotSorted,
    InconsistentSigners,
    InvalidHash,
    FullDataHash,
    UndecodableMessageData,
    EventMessage,
    UnknownSSVMessageType,
    UnknownQBFTMessageType,
    InvalidPartialSignatureType,
    PartialSignatureTypeRoleMismatch,
    NonDecidedWithMultipleSigners {
        got: usize,
        want: usize,
    },
    DecidedNotEnoughSigners {
        got: usize,
        want: usize,
    },
    DifferentProposalData,
    MalformedPrepareJustifications,
    UnexpectedPrepareJustifications,
    MalformedRoundChangeJustifications,
    UnexpectedRoundChangeJustifications,
    NoPartialSignatureMessages,
    NoValidators,
    NoSignatures,
    SignersAndSignaturesWithDifferentLength,
    PartialSigOneSigner,
    PrepareOrCommitWithFullData,
    FullDataNotInConsensusMessage,
    TripleValidatorIndexInPartialSignatures,
    ZeroRound,
    DuplicatedMessage {
        got: String,
    }, // Updated to include context
    InvalidPartialSignatureTypeCount,
    TooManyPartialSignatureMessages,
    EncodeOperators,
    FailedToGetMaxRound,
    SlotStartTimeNotFound,
}

impl From<&ValidationFailure> for MessageAcceptance {
    fn from(value: &ValidationFailure) -> Self {
        match value {
            ValidationFailure::WrongDomain
            | ValidationFailure::NoShareMetadata
            | ValidationFailure::UnknownValidator
            | ValidationFailure::ValidatorLiquidated
            | ValidationFailure::ValidatorNotAttesting
            | ValidationFailure::EarlySlotMessage
            | ValidationFailure::LateSlotMessage
            | ValidationFailure::SlotAlreadyAdvanced
            | ValidationFailure::RoundAlreadyAdvanced { .. }
            | ValidationFailure::DecidedWithSameSigners
            | ValidationFailure::PubSubDataTooBig(_)
            | ValidationFailure::IncorrectTopic
            | ValidationFailure::NonExistentCommitteeID
            | ValidationFailure::RoundTooHigh
            | ValidationFailure::ValidatorIndexMismatch
            | ValidationFailure::TooManyDutiesPerEpoch
            | ValidationFailure::NoDuty
            | ValidationFailure::EstimatedRoundNotInAllowedSpread { .. } => {
                MessageAcceptance::Ignore
            }
            _ => MessageAcceptance::Reject,
        }
    }
}

#[derive(Debug)]
pub enum ValidatedSSVMessage {
    QbftMessage(QbftMessage),
    PartialSignatureMessages(PartialSignatureMessages),
}

#[derive(Debug)]
pub struct ValidatedMessage {
    pub signed_ssv_message: SignedSSVMessage,
    pub ssv_message: ValidatedSSVMessage,
}

impl ValidatedMessage {
    pub fn new(signed_ssv_message: SignedSSVMessage, ssv_message: ValidatedSSVMessage) -> Self {
        Self {
            signed_ssv_message,
            ssv_message,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] ::processor::Error),
}

#[derive(Clone)]
pub struct Validator<S: SlotClock> {
    network_state_rx: Receiver<NetworkState>,
    consensus_state_map: DashMap<MessageId, Arc<Mutex<ConsensusState>>>,
    slots_per_epoch: u64,
    slot_clock: S,
}

impl<S: SlotClock> Validator<S> {
    pub fn new(
        network_state_rx: Receiver<NetworkState>,
        slots_per_epoch: u64,
        slot_clock: S,
    ) -> Self {
        Self {
            network_state_rx,
            consensus_state_map: DashMap::new(),
            slots_per_epoch,
            slot_clock,
        }
    }

    pub fn validate(&self, message_data: &[u8]) -> Result<ValidatedMessage, ValidationFailure> {
        match SignedSSVMessage::from_ssz_bytes(message_data) {
            Ok(signed_ssv_message) => {
                trace!(msg = ?signed_ssv_message, "SignedSSVMessage deserialized");

                // Get the role from message ID
                let ssv_message = signed_ssv_message.ssv_message();
                let role = ssv_message
                    .msg_id()
                    .role()
                    .ok_or(ValidationFailure::InvalidRole)?;

                // Get committee info based on role and duty executor
                let network_state = self.network_state_rx.borrow();
                let committee_info = match role {
                    Role::Committee => {
                        let committee_id = match ssv_message.msg_id().duty_executor() {
                            Some(DutyExecutor::Committee(id)) => id,
                            _ => return Err(ValidationFailure::NonExistentCommitteeID),
                        };
                        network_state
                            .get_committee_info_by_committee_id(&committee_id)
                            .ok_or(ValidationFailure::NonExistentCommitteeID)?
                    }
                    _ => {
                        let validator_pk = match ssv_message.msg_id().duty_executor() {
                            Some(DutyExecutor::Validator(pk)) => pk,
                            _ => return Err(ValidationFailure::UnknownValidator),
                        };

                        network_state
                            .get_committee_info_by_validator_pk(&validator_pk)
                            .ok_or(ValidationFailure::UnknownValidator)?
                    }
                };
                let consensus_state_arc =
                    self.get_consensus_state(ssv_message.msg_id(), self.slots_per_epoch);
                let mut consensus_state = consensus_state_arc.lock();
                validate_ssv_message(
                    &signed_ssv_message,
                    &committee_info,
                    role,
                    &mut consensus_state,
                    self.slots_per_epoch,
                    self.slot_clock.clone(),
                )
                .map(|validated| ValidatedMessage::new(signed_ssv_message.clone(), validated))
            }
            Err(error) => {
                trace!("error" = ?error, "Failed to deserialize SignedSSVMessage");
                Err(ValidationFailure::UndecodableMessageData)
            }
        }
    }

    /// Gets the consensus state for a message ID, creating a new one if it doesn't exist
    fn get_consensus_state(
        &self,
        message_id: &MessageId,
        slots_per_epoch: u64,
    ) -> Arc<Mutex<ConsensusState>> {
        self.consensus_state_map
            .entry(message_id.clone())
            .or_insert_with(|| {
                let stored_slot_count = slots_per_epoch * 2; // Store last two epochs

                Arc::new(Mutex::new(ConsensusState::new(stored_slot_count as usize)))
            })
            .clone()
    }
}

fn validate_ssv_message(
    signed_ssv_message: &SignedSSVMessage,
    committee_info: &CommitteeInfo,
    role: Role,
    consensus_state: &mut ConsensusState,
    slots_per_epoch: u64,
    slot_clock: impl SlotClock,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    let ssv_message = signed_ssv_message.ssv_message();
    let received_at = SystemTime::now();

    match ssv_message.msg_type() {
        MsgType::SSVConsensusMsgType => validate_consensus_message(
            signed_ssv_message,
            ssv_message,
            committee_info,
            role,
            consensus_state,
            received_at,
            slots_per_epoch,
            slot_clock,
        ),
        MsgType::SSVPartialSignatureMsgType => validate_partial_signature_message(
            signed_ssv_message,
            ssv_message,
            committee_info,
            role,
        ),
    }
}

pub(crate) fn compute_quorum_size(committee_size: usize) -> usize {
    let f = get_f(committee_size);
    f * 2 + 1
}

// # TODO centralize this and the one in the qbft crate
pub(crate) fn get_f(committee_size: usize) -> usize {
    (committee_size - 1) / 3
}

pub(crate) fn hash_data(full_data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(full_data);
    let hash: [u8; 32] = hasher.finalize().into();
    hash
}

#[cfg(test)]
mod tests {
    use bls::PublicKeyBytes;
    use ssv_types::{
        domain_type::DomainType,
        msgid::{DutyExecutor, MessageId, Role},
        CommitteeId, CommitteeInfo, IndexSet, OperatorId, ValidatorIndex,
    };

    use crate::{compute_quorum_size, hash_data, ValidationFailure};

    // Constants for committee sizes in tests to improve readability
    pub(crate) const SINGLE_NODE_COMMITTEE: usize = 1;
    pub(crate) const FOUR_NODE_COMMITTEE: usize = 4;
    pub(crate) const SEVEN_NODE_COMMITTEE: usize = 7;

    // Create a committee info object for tests
    pub(crate) fn create_committee_info(committee_size: usize) -> CommitteeInfo {
        let mut members = IndexSet::new();
        for i in 0..committee_size {
            // Start from 1 to avoid zero values
            members.insert(OperatorId(i as u64 + 1));
        }

        CommitteeInfo {
            committee_members: members,
            validator_indices: vec![ValidatorIndex(0), ValidatorIndex(123)],
        }
    }

    // Helper to create a message ID for tests
    pub fn create_message_id_for_test(role: Role) -> MessageId {
        let domain = DomainType([0, 0, 0, 1]);
        let duty_executor = match role {
            Role::Committee => DutyExecutor::Committee(CommitteeId([0u8; 32])),
            _ => DutyExecutor::Validator(PublicKeyBytes::empty()),
        };
        MessageId::new(&domain, role, &duty_executor)
    }

    // Assert helpers for common validation patterns
    pub fn assert_validation_error<T, F>(
        result: Result<T, ValidationFailure>,
        expected_error: F,
        error_name: &str,
    ) where
        F: Fn(&ValidationFailure) -> bool,
    {
        match result {
            Ok(_) => panic!("Expected validation to fail with {}", error_name),
            Err(failure) => {
                assert!(
                    expected_error(&failure),
                    "Expected {} error, got: {:?}",
                    error_name,
                    failure
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // Utility function tests
    // ---------------------------------------------------------------------

    #[test]
    fn test_compute_quorum_size() {
        // For committee_size=4 -> f=1 -> quorum=3.
        assert_eq!(
            compute_quorum_size(FOUR_NODE_COMMITTEE),
            3,
            "Expected quorum=3 for committee of 4"
        );
        // For committee_size=7 -> f=2 -> quorum=5.
        assert_eq!(
            compute_quorum_size(SEVEN_NODE_COMMITTEE),
            5,
            "Expected quorum=5 for committee of 7"
        );
        // For committee_size=1 -> f=0 -> quorum=1.
        assert_eq!(
            compute_quorum_size(SINGLE_NODE_COMMITTEE),
            1,
            "Expected quorum=1 for committee of 1"
        );
    }

    #[test]
    fn test_hash_data_root() {
        let data1 = vec![1, 2, 3, 4];
        let data2 = vec![1, 2, 3, 5]; // One byte different

        let hash1 = hash_data(&data1);
        let hash2 = hash_data(&data2);

        assert_ne!(
            hash1, hash2,
            "Different data should produce different hashes"
        );
        assert_eq!(
            hash1,
            hash_data(&data1),
            "Same data should produce the same hash"
        );
    }
}
