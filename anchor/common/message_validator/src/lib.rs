use crate::Result::Accept;
use ssv_types::message::SignedSSVMessage;
use std::time::Duration;

// TODO taken from go-SSV as rough guidance. feel free to adjust as needed. https://github.com/ssvlabs/ssv/blob/e12abf7dfbbd068b99612fa2ebbe7e3372e57280/message/validation/errors.go#L55
#[derive(Clone)]
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

impl From<&ValidationFailure> for Result {
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
            | ValidationFailure::EstimatedRoundNotInAllowedSpread => Result::Ignore(value.clone()),
            _ => Result::Reject(value.clone()),
        }
    }
}

pub enum Result {
    Accept,
    Reject(ValidationFailure),
    Ignore(ValidationFailure),
}

use processor::Senders;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::timeout;
use tracing::{error, warn};

pub struct Validator {
    processor: Senders,
}

pub(crate) fn validate(_message: SignedSSVMessage) -> Result {
    Accept
}

pub fn start_validator_service(
    validator: Validator,
    channel_capacity: usize,
    send_timeout: Duration,
) -> (Sender<SignedSSVMessage>, Receiver<Result>) {
    let (msg_tx, mut msg_rx) = mpsc::channel::<SignedSSVMessage>(channel_capacity);
    let (result_tx, result_rx) = mpsc::channel::<Result>(channel_capacity);

    match validator.processor.urgent_consensus.send_async(
        async move {
            while let Some(signed_msg) = msg_rx.recv().await {
                let result = validate(signed_msg);

                // Wrap the send in a timeout.
                match timeout(send_timeout, result_tx.send(result)).await {
                    Ok(Ok(_)) => { /* successful send */ }
                    Ok(Err(_)) => {
                        error!("Validation result receiver dropped");
                        break;
                    }
                    Err(_) => {
                        warn!("Timed out sending validation result");
                        // metrics::inc_counter_vec(
                        //     &metrics::VALIDATOR_RESULT_TIMEOUTS,
                        //     &["validator_service"],
                        // );
                    }
                }
            }
        },
        "validator_service",
    ) {
        Ok(_) => { /* Service started successfully */ }
        Err(error) => {
            error!(?error, "Failed to schedule the validator service");
            // Here we can decide to either propagate the error or take corrective action.
            // For example, we might return an error
            // return Err(Error::ValidatorService(e));
        }
    }

    (msg_tx, result_rx)
}
