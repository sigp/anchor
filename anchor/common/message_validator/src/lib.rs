use ssv_types::message::SignedSSVMessage;
use std::sync::Arc;

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

impl From<&ValidationFailure> for Action {
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
            | ValidationFailure::EstimatedRoundNotInAllowedSpread => Action::Ignore,
            _ => Action::Reject,
        }
    }
}

pub enum Action {
    Accept,
    Reject,
    Ignore,
}

pub struct Result {
    pub message_id: u64,
    pub message: SignedSSVMessage,
    pub action: Action,
}

impl Result {
    pub fn new(message_id: u64, message: SignedSSVMessage, action: Action) -> Self {
        Self {
            message_id,
            message,
            action,
        }
    }
}

use processor::Senders;
use tokio::sync::mpsc::error::TrySendError::{Closed, Full};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tracing::{error, trace};
use crate::Action::Accept;

pub struct Validator {
    processor: Senders,
    result_tx: Sender<Result>,
    result_rx: Receiver<Result>,
}

pub trait ValidatorService {
    fn validation_result_rx(&mut self) -> &mut Receiver<Result>;

    fn validate(self: Arc<Self>, message_id: u64, message: SignedSSVMessage);
}


impl Validator {
    pub fn new(processor: Senders, channel_capacity: usize) -> Self {
        let (result_tx, result_rx) = mpsc::channel(channel_capacity);
        Self {
            processor,
            result_tx,
            result_rx,
        }
    }

    fn do_validate(&self, _message: &SignedSSVMessage) -> std::result::Result<(), ValidationFailure> {
        Err(ValidationFailure::DecidedNotEnoughSigners)
    }
}

impl ValidatorService for Validator {
    fn validation_result_rx(&mut self) -> &mut Receiver<Result> {
        &mut self.result_rx
    }

    fn validate(self: Arc<Self>, message_id: u64, message: SignedSSVMessage) {
        let validator = self.clone();
        match self.processor.urgent_consensus.send_blocking(
            move || {
                let result = match validator.do_validate(&message) {
                    Ok(()) => Accept,
                    Err(failure) => {
                        trace!(?failure, message_id, "Validation failure");
                        (&failure).into()
                    }
                };
                match validator.result_tx.try_send(Result::new(message_id, message, result)) {
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
        ) {
            Ok(_) => {
                trace!("Validation task scheduled");
            }
            Err(error) => {
                error!(?error, "Failed to schedule the validator Task");
                // Here we can decide to either propagate the error or take corrective action.
                // For example, we might return an error
                // return Err(Error::ValidatorService(e));
            }
        }
    }
}