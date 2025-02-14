use crate::types::ssv_message::SignedSSVMessage;
use crate::Network;
use libp2p::gossipsub::{Message, MessageAcceptance};
use ssz::Decode;
use tracing::debug;

// TODO taken from go-SSV as rough guidance. feel free to adjust as needed. https://github.com/ssvlabs/ssv/blob/e12abf7dfbbd068b99612fa2ebbe7e3372e57280/message/validation/errors.go#L55
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
    MalformedPubSubMessage(ssz::DecodeError),
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

impl Network {
    pub(crate) fn validate_message(
        &mut self,
        message: Message,
    ) -> Result<SignedSSVMessage, ValidationFailure> {
        // todo pre-deserialization validation

        let message = SignedSSVMessage::from_ssz_bytes(&message.data)
            .map_err(ValidationFailure::MalformedPubSubMessage)?;
        debug!(msg = ?message, "SignedSSVMessage deserialized");

        // todo post-deserialization validation

        Ok(message)
    }
}
