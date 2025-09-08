use crate::qbft::adapters::spec_types::TestMessageConversionError;
use message_validator::ValidationFailure;
use qbft::QbftError;
use ssv_types::consensus::QbftValidationError;
use ssv_types::message::SignedSSVMessageError;
use ssz::DecodeError;

/// Maps QbftError to spec test error strings
pub fn map_qbft_error(error: &QbftError) -> Vec<String> {
    match error {
        // Message validation errors
        QbftError::SignerNotInCommittee => vec!["invalid signed message: signer not in committee".to_string()],
        QbftError::WrongHeight => vec!["invalid signed message: wrong msg height".to_string()],
        QbftError::WrongRound => vec!["invalid signed message: wrong msg round".to_string()],
        QbftError::PastRound => vec!["invalid signed message: past round".to_string()],
        QbftError::InvalidFullData => vec!["invalid signed message: H(data) != root".to_string()],

        // Proposal specific
        QbftError::ProposalNotFromLeader => vec!["invalid signed message: proposal leader invalid".to_string()],
        QbftError::ProposalAlreadyReceived => vec!["invalid signed message: proposal already received".to_string()],
        QbftError::ProposalMissingData => vec!["invalid signed message: H(data) != root".to_string()],

        // Round change justifications
        QbftError::ProposalRoundChangeJustificationNoQuorum =>
          vec!["invalid signed message: proposal not justified: change round has no quorum".to_string()],
        QbftError::RoundChangeJustificationNoQuorum =>
            vec!["invalid signed message: no justifications quorum".to_string()],
        QbftError::RoundChangeJustificationWrongRound =>
            vec!["invalid signed message: round change justification invalid: wrong msg round".to_string()],
        QbftError::RoundChangeJustificationDecodeFailed =>
            vec!["invalid signed message: round change justification invalid: decode failed".to_string()],
        QbftError::RoundChangeJustificationNotRoundChange =>
            vec!["invalid signed message: round change justification invalid: not a round change".to_string()],
        QbftError::RoundChangeJustificationInvalidPrepareRoot =>
            vec!["invalid signed message: proposal not justified: change round msg not valid: round change justification invalid: proposed data mismatch".to_string()],
        QbftError::RoundChangeJustificationMultiSigner =>
            vec!["invalid signed message: round change justification invalid: msg allows 1 signer".to_string()],
        QbftError::RoundChangeJustificationNoPrepareQuorum =>
            vec!["invalid signed message: proposal not justified: change round msg not valid: no justifications quorum".to_string()],

        // Prepare justifications
        QbftError::PrepareJustificationMultiSigner =>
            vec!["invalid signed message: round change justification invalid: msg allows 1 signer".to_string()],
        QbftError::PrepareJustificationWrongRound =>
            vec![
                "invalid signed message: round change justification invalid: wrong msg round".to_string(),
                "invalid signed message: proposal not justified: change round msg not valid: round change justification invalid: wrong msg round".to_string(),
                "invalid signed message: proposal not justified: signed prepare not valid".to_string(),
            ],
        QbftError::PrepareJustificationDecodeFailed =>
            vec!["invalid signed message: prepare justification invalid: decode failed".to_string()],
        QbftError::PrepareJustificationNotPrepare =>
            vec!["invalid signed message: prepare justification invalid: not a prepare".to_string()],
        QbftError::PrepareJustificationRootMismatch =>
            vec!["invalid signed message: proposal not justified: change round msg not valid: round change justification invalid: proposed data mismatch".to_string()],

        // State errors
        QbftError::InvalidState => vec!["invalid signed message: proposal is not valid with current state".to_string()],
        QbftError::ProposedDataMismatch => vec!["invalid signed message: proposed data mismatch".to_string()],
        QbftError::ProposalNotAccepted => vec!["invalid signed message: did not receive proposal for this round".to_string()],

        _ => vec!["todo".to_string()]
    }
}

/// Maps our internal SignedSSVMessageError to the expected error strings from Go spec tests
pub fn map_signed_message_error(error: &SignedSSVMessageError) -> String {
    match error {
        SignedSSVMessageError::NoSigners => {
            "invalid signed message: invalid SignedSSVMessage: no signers".to_string()
        }
        SignedSSVMessageError::DuplicatedSigner => {
            "invalid signed message: invalid SignedSSVMessage: non unique signer".to_string()
        }
        SignedSSVMessageError::ZeroSigner => {
            "invalid signed message: invalid SignedSSVMessage: signer ID 0 not allowed".to_string()
        }
        SignedSSVMessageError::SignersNotSorted => {
            "invalid signed message: invalid SignedSSVMessage: signers not sorted".to_string()
        }
        _ => format!("Failed to create SignedSSVMessage: {:?}", error),
    }
}

/// Error types specific to qbft_message tests
#[derive(Debug, Clone)]
pub enum QbftMessageError {
    SignedMessageError(SignedSSVMessageError),
    ConversionError(crate::qbft::adapters::spec_types::TestMessageConversionError),
    SSZDecodeError(ssz::DecodeError),
    Validation(QbftValidationError),
}

/// Map QbftMessageError to the expected error string for test comparison
pub fn map_qbft_message_error(error: &QbftMessageError) -> String {
    match error {
        QbftMessageError::SignedMessageError(e) => {
            // Map actual SignedSSVMessageError variants to expected strings
            match e {
                SignedSSVMessageError::NoSigners => "no signers".to_string(),
                SignedSSVMessageError::DuplicatedSigner => "non unique signer".to_string(),
                SignedSSVMessageError::ZeroSigner => "signer ID 0 not allowed".to_string(),
                SignedSSVMessageError::SignersNotSorted => "signers not sorted".to_string(),
                SignedSSVMessageError::NoSignatures => "no signatures".to_string(),
                SignedSSVMessageError::TooManySignatures { .. } => {
                    "too many signatures".to_string()
                }
                SignedSSVMessageError::WrongRSASignatureSize { .. } => {
                    "wrong signature size".to_string()
                }
                SignedSSVMessageError::TooManyOperatorIDs { .. } => {
                    "too many operators".to_string()
                }
                SignedSSVMessageError::FullDataTooLong { .. } => "full data too long".to_string(),
                SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => {
                    "signers signatures length mismatch".to_string()
                }
                SignedSSVMessageError::SSVMessageError(_) => "ssv message error".to_string(),
            }
        }
        QbftMessageError::ConversionError(e) => {
            // Map TestMessageConversionError to expected strings
            match e {
                TestMessageConversionError::SignedSSVMessage(ssv_err) => {
                    // Reuse the same mapping for nested SignedSSVMessageError
                    match ssv_err {
                        SignedSSVMessageError::NoSigners => "no signers".to_string(),
                        SignedSSVMessageError::DuplicatedSigner => "non unique signer".to_string(),
                        SignedSSVMessageError::ZeroSigner => "signer ID 0 not allowed".to_string(),
                        SignedSSVMessageError::SignersNotSorted => "signers not sorted".to_string(),
                        SignedSSVMessageError::NoSignatures => "no signatures".to_string(),
                        SignedSSVMessageError::TooManySignatures { .. } => {
                            "too many signatures".to_string()
                        }
                        SignedSSVMessageError::WrongRSASignatureSize { .. } => {
                            "wrong signature size".to_string()
                        }
                        SignedSSVMessageError::TooManyOperatorIDs { .. } => {
                            "too many operators".to_string()
                        }
                        SignedSSVMessageError::FullDataTooLong { .. } => {
                            "full data too long".to_string()
                        }
                        SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => {
                            "signers signatures length mismatch".to_string()
                        }
                        SignedSSVMessageError::SSVMessageError(_) => {
                            "ssv message error".to_string()
                        }
                    }
                }
                TestMessageConversionError::Base64Decode(_) => "invalid base64".to_string(),
                TestMessageConversionError::InvalidSignatureLength { .. } => {
                    "incorrect size".to_string()
                }
                TestMessageConversionError::SSZDecode(_) => "message data is invalid".to_string(),
                TestMessageConversionError::MissingSSVMessage => "missing ssv message".to_string(),
                TestMessageConversionError::InvalidFullData(_) => "invalid full data".to_string(),
                TestMessageConversionError::MultiSignerNotAllowed => {
                    "msg allows 1 signer".to_string()
                }
            }
        }
        QbftMessageError::SSZDecodeError(e) => match e {
            DecodeError::NoMatchingVariant => "message type is invalid".to_string(),
            _ => "not tested".to_string(),
        },
        QbftMessageError::Validation(e) => match e {
            QbftValidationError::InvalidIdentifier => "message identifier is invalid".to_string(),
            QbftValidationError::InvalidJustifications => "incorrect size".to_string(),
            _ => "not tested".to_string(),
        },
    }
}

pub fn map_validation_error(error: ValidationFailure) -> String {
    match error {
        ValidationFailure::SignerNotInCommittee => {
            "invalid decided msg: invalid decided msg: signer not in committee".to_string()
        }
        ValidationFailure::NonDecidedWithMultipleSigners { .. } => {
            "could not process msg: invalid signed message: msg allows 1 signer".to_string()
        }
        _ => "not mapped".to_string(),
    }
}
