extern crate core;

mod consensus_message;
mod partial_signature;

use crate::consensus_message::validate_consensus_message_semantics;
use crate::partial_signature::validate_partial_signature_message;
use database::NetworkState;
use gossipsub::MessageAcceptance;
use sha2::{Digest, Sha256};
use ssv_types::consensus::QbftMessage;
use ssv_types::message::{MsgType, SignedSSVMessage};
use ssv_types::msgid::{DutyExecutor, Role};
use ssv_types::partial_sig::PartialSignatureMessages;
use ssv_types::CommitteeInfo;
use ssz::Decode;
use tokio::sync::watch::Receiver;
use tracing::{error, trace};

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
    RoundAlreadyAdvanced,
    DecidedWithSameSigners,
    PubSubDataTooBig(usize),
    IncorrectTopic,
    NonExistentCommitteeID,
    RoundTooHigh,
    ValidatorIndexMismatch,
    TooManyDutiesPerEpoch,
    NoDuty,
    EstimatedRoundNotInAllowedSpread,
    EmptyData,
    MismatchedIdentifier { got: String, want: String },
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
    SignerNotLeader,
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
    NonDecidedWithMultipleSigners { got: usize, want: usize },
    DecidedNotEnoughSigners { got: usize, want: usize },
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
    DuplicatedMessage,
    InvalidPartialSignatureTypeCount,
    TooManyPartialSignatureMessages,
    EncodeOperators,
    FailedToGetMaxRound,
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
            | ValidationFailure::RoundAlreadyAdvanced
            | ValidationFailure::DecidedWithSameSigners
            | ValidationFailure::PubSubDataTooBig(_)
            | ValidationFailure::IncorrectTopic
            | ValidationFailure::NonExistentCommitteeID
            | ValidationFailure::RoundTooHigh
            | ValidationFailure::ValidatorIndexMismatch
            | ValidationFailure::TooManyDutiesPerEpoch
            | ValidationFailure::NoDuty
            | ValidationFailure::EstimatedRoundNotInAllowedSpread => MessageAcceptance::Ignore,
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

pub struct Validator {
    network_state_rx: Receiver<NetworkState>,
}

impl Validator {
    pub fn new(network_state_rx: Receiver<NetworkState>) -> Self {
        Self { network_state_rx }
    }

    pub fn validate(&self, message_data: Vec<u8>) -> Result<ValidatedMessage, ValidationFailure> {
        match SignedSSVMessage::from_ssz_bytes(&message_data) {
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

                validate_ssv_message(&signed_ssv_message, &committee_info, role)
                    .map(|validated| ValidatedMessage::new(signed_ssv_message.clone(), validated))
            }
            Err(error) => {
                trace!("error" = ?error, "Failed to deserialize SignedSSVMessage");
                Err(ValidationFailure::UndecodableMessageData)
            }
        }
    }
}

fn validate_ssv_message(
    signed_ssv_message: &SignedSSVMessage,
    committee_info: &CommitteeInfo,
    role: Role,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    let ssv_message = signed_ssv_message.ssv_message();

    match ssv_message.msg_type() {
        MsgType::SSVConsensusMsgType => {
            let consensus_message = QbftMessage::from_ssz_bytes(ssv_message.data())
                .ok()
                .ok_or(ValidationFailure::UndecodableMessageData)?;
            validate_consensus_message_semantics(
                signed_ssv_message,
                &consensus_message,
                committee_info,
            )?;
            Ok(ValidatedSSVMessage::QbftMessage(consensus_message))
        }
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
    use crate::{compute_quorum_size, hash_data, ValidationFailure};
    use bls::PublicKeyBytes;
    use ssv_types::domain_type::DomainType;
    use ssv_types::msgid::{DutyExecutor, MessageId, Role};
    use ssv_types::{CommitteeId, CommitteeInfo, IndexSet, OperatorId, ValidatorIndex};

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
