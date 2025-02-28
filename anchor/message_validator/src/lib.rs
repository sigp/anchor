use libp2p::gossipsub::MessageAcceptance::{Accept, Reject};
use libp2p::gossipsub::{MessageAcceptance, MessageId};
use libp2p::PeerId;
use processor::Senders;
use ssv_types::message::SignedSSVMessage;
use ssz::Decode;
use std::result;
use std::sync::Arc;
use tokio::sync::mpsc::error::TrySendError::{Closed, Full};
use tokio::sync::mpsc::Sender;
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
    MismatchedIdentifier,
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
    NonDecidedWithMultipleSigners,
    DecidedNotEnoughSigners,
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

pub struct Result {
    pub message_id: MessageId,
    pub propagation_source: PeerId,
    pub message: Option<SignedSSVMessage>,
    pub action: MessageAcceptance,
}

impl Result {
    pub fn new(
        message_id: MessageId,
        propagation_success: PeerId,
        message: Option<SignedSSVMessage>,
        action: MessageAcceptance,
    ) -> Self {
        Self {
            message_id,
            propagation_source: propagation_success,
            message,
            action,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] ::processor::Error),
}

pub struct Validator {
    processor: Senders,
    result_tx: Sender<Result>,
}

pub trait ValidatorService {
    fn validate(
        self: Arc<Self>,
        message_id: MessageId,
        propagation_source: PeerId,
        message_data: Vec<u8>,
    ) -> result::Result<(), Error>;
}

impl Validator {
    pub fn new(processor: Senders, result_tx: Sender<Result>) -> Self {
        Self {
            processor,
            result_tx,
        }
    }

    fn do_validate(&self, _message: &SignedSSVMessage) -> result::Result<(), ValidationFailure> {
        Ok(())
    }
}

impl ValidatorService for Validator {
    fn validate(
        self: Arc<Self>,
        message_id: MessageId,
        propagation_source: PeerId,
        message_data: Vec<u8>,
    ) -> result::Result<(), Error> {
        let validator = self.clone();
        Ok(self.processor.urgent_consensus.send_blocking(
            move || {
                let (result, msg) = match SignedSSVMessage::from_ssz_bytes(&message_data) {
                    Ok(deserialized_message) => {
                        trace!(msg = ?deserialized_message, "SignedSSVMessage deserialized");
                        match validator.do_validate(&deserialized_message) {
                            Ok(()) => (Accept, Some(deserialized_message)),
                            Err(failure) => {
                                trace!(
                                    ?failure,
                                    ?message_id,
                                    ?propagation_source,
                                    "Validation failure"
                                );
                                ((&failure).into(), Some(deserialized_message))
                            }
                        }
                    }
                    Err(error) => {
                        trace!("error" = ?error, "Failed to deserialize SignedSSVMessage");
                        (Reject, None)
                    }
                };
                match validator.result_tx.try_send(Result::new(
                    message_id,
                    propagation_source,
                    msg,
                    result,
                )) {
                    Ok(()) => (),
                    Err(Closed(_)) => {
                        error!("Validation result receiver dropped");
                    }
                    Err(Full(_)) => {
                        error!("Validation result receiver full");
                        // metrics::inc_counter_vec(
                        //     &metrics::VALIDATOR_RESULT_TIMEOUTS,
                        //     &["validator_service"],
                        // );
                    }
                }
            },
            "validator",
        )?)
    }
}
