//! Go error codes from ssv-spec `types/error.go` (iota + 1).
//!
//! Only codes relevant to currently implemented spec tests are included.
//! Add new codes as new test types are implemented.

use ssv_types::{consensus::AggregatorCommitteeValidationError, message::SignedSSVMessageError};

/// No error — validation passed.
pub const NO_ERROR: i64 = 0;

// --- SignedSSVMessage.Validate() codes ---

/// `NonUniqueSignerErrorCode` (iota 8 → value 9)
pub const NON_UNIQUE_SIGNER: i64 = 9;

/// `IncorrectNumberOfSignaturesErrorCode` (iota 11 → value 12)
pub const INCORRECT_NUMBER_OF_SIGNATURES: i64 = 12;

/// `EmptySignatureErrorCode` (iota 12 → value 13)
pub const EMPTY_SIGNATURE: i64 = 13;

/// `NilSSVMessageErrorCode` (iota 13 → value 14)
pub const NIL_SSV_MESSAGE: i64 = 14;

/// `NoSignaturesErrorCode` (iota 14 → value 15)
pub const NO_SIGNATURES: i64 = 15;

/// `NoSignersErrorCode` (iota 15 → value 16)
pub const NO_SIGNERS: i64 = 16;

/// `ZeroSignerNotAllowedErrorCode` (iota 16 → value 17)
pub const ZERO_SIGNER_NOT_ALLOWED: i64 = 17;

// --- PartialSignatureMessages.Validate() codes ---

/// `InconsistentSignersErrorCode` (iota 17 → value 18)
pub const INCONSISTENT_SIGNERS: i64 = 18;

/// `NoPartialSigMessagesErrorCode` (iota 18 → value 19)
pub const NO_PARTIAL_SIG_MESSAGES: i64 = 19;

// --- RSA signature verification codes ---

/// `SSVMessageHasInvalidSignatureErrorCode` (iota 38 → value 39)
pub const SSV_MESSAGE_HAS_INVALID_SIGNATURE: i64 = 39;

// --- ProposerConsensusData / GetBlockData codes ---

/// `UnmarshalSSZErrorCode` (iota 0 → value 1) — SSZ decode failure.
pub const UNMARSHAL_SSZ: i64 = 1;

/// `UnknownDutyRoleDataErrorCode` (iota 9 → value 10) — duty type is not `BNRoleProposer`.
pub const UNKNOWN_DUTY_ROLE_DATA: i64 = 10;

/// `UnknownBlockVersionErrorCode` (iota 10 → value 11) — unrecognized fork version.
pub const UNKNOWN_BLOCK_VERSION: i64 = 11;

// --- AggregatorCommitteeConsensusData.Validate() codes ---

/// `AggCommAggCommIdxCntMismatchErrorCode` (iota 70 → value 71)
pub const AGG_COMM_INDEX_COUNT_MISMATCH: i64 = 71;

/// `AggCommCommIdxMismatchErrorCode` (iota 71 → value 72)
pub const AGG_COMM_INDEX_MISSING: i64 = 72;

/// `AggCommUnusedCommIdxErrorCode` (iota 72 → value 73)
pub const AGG_COMM_INDEX_UNUSED: i64 = 73;

/// `AggCommDuplicatedCommIdxErrorCode` (iota 73 → value 74)
pub const AGG_COMM_INDEX_DUPLICATE: i64 = 74;

/// `AggCommSubnetNotInSCSubnetsErrorCode` (iota 74 → value 75)
pub const AGG_COMM_SC_SUBNET_MISSING: i64 = 75;

/// `AggCommSCCSubnetDuplicateErrorCode` (iota 75 → value 76)
pub const AGG_COMM_SC_SUBNET_DUPLICATE: i64 = 76;

/// `AggCommUnusedSubnetErrorCode` (iota 76 → value 77)
pub const AGG_COMM_SC_SUBNET_UNUSED: i64 = 77;

/// `AggCommConsensusDataNoValidatorErrorCode` (iota 77 → value 78)
pub const AGG_COMM_NO_VALIDATORS: i64 = 78;

/// `AggCommAttestationDecodingErrorCode` (iota 83 → value 84)
pub const AGG_COMM_ATTESTATION_DECODE: i64 = 84;

// --- Sentinel ---

/// Sentinel for Anchor-specific errors without Go equivalents.
pub const UNMAPPED_ERROR_CODE: i64 = -1;

// --- Mappers ---

/// Maps `SignedSSVMessageError` variants to Go's integer error codes from ssv-spec.
///
/// Variants without a Go equivalent map to `UNMAPPED_ERROR_CODE` so tests fail loudly
/// on mismatch rather than silently matching some unrelated code.
pub fn signed_ssv_message_error_code(err: &SignedSSVMessageError) -> i64 {
    match err {
        SignedSSVMessageError::NoSigners => NO_SIGNERS,
        SignedSSVMessageError::NoSignatures => NO_SIGNATURES,
        SignedSSVMessageError::ZeroSigner => ZERO_SIGNER_NOT_ALLOWED,
        SignedSSVMessageError::DuplicatedSigner => NON_UNIQUE_SIGNER,
        SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => {
            INCORRECT_NUMBER_OF_SIGNATURES
        }
        // Unreachable when callers pad all sigs to exactly `[u8; 256]` before
        // `SignedSSVMessage::new()`, so the size check inside `new()` always passes.
        SignedSSVMessageError::WrongRSASignatureSize { .. } => EMPTY_SIGNATURE,
        SignedSSVMessageError::TooManySignatures { .. }
        | SignedSSVMessageError::TooManyOperatorIDs { .. }
        | SignedSSVMessageError::FullDataTooLong { .. }
        | SignedSSVMessageError::SignersNotSorted
        | SignedSSVMessageError::SSVMessageError(_) => UNMAPPED_ERROR_CODE,
    }
}

/// Maps `AggregatorCommitteeValidationError` variants to Go's integer error codes.
///
/// Exhaustive match: a new variant on the production enum will fail to compile here,
/// preventing silent drift to `UNMAPPED_ERROR_CODE`.
pub fn aggregator_committee_validation_error_code(err: AggregatorCommitteeValidationError) -> i64 {
    use AggregatorCommitteeValidationError as E;
    match err {
        E::CommitteeIndexCountMismatch { .. } => AGG_COMM_INDEX_COUNT_MISMATCH,
        E::DuplicateCommitteeIndex(_) => AGG_COMM_INDEX_DUPLICATE,
        E::AggregatorCommitteeIndexMissing(_) => AGG_COMM_INDEX_MISSING,
        E::AggregatorCommitteeUnusedIndex => AGG_COMM_INDEX_UNUSED,
        E::DuplicateSyncSubcommittee(_) => AGG_COMM_SC_SUBNET_DUPLICATE,
        E::ContributorSubcommitteeMissing(_) => AGG_COMM_SC_SUBNET_MISSING,
        E::SyncSubcommitteeUnusedIndex => AGG_COMM_SC_SUBNET_UNUSED,
        E::NoValidatorsAssigned => AGG_COMM_NO_VALIDATORS,
        E::AttestationDecodeError(_) => AGG_COMM_ATTESTATION_DECODE,
    }
}

// --- Assertions ---

/// Mirrors ssv-spec's `tests.AssertErrorCode` (`types/spectest/tests/error.go`):
/// `expected_error_code == NO_ERROR` requires `actual_code == NO_ERROR`;
/// `actual_code == UNMAPPED_ERROR_CODE` always fails loudly (matches Go's
/// `errors.As(err, &types.Error)` guard, which fails with "unknown error" when
/// the error isn't a typed spec error); otherwise the codes are compared for equality.
pub fn assert_error_code(expected_error_code: i64, actual_code: i64) -> Result<(), String> {
    if actual_code == UNMAPPED_ERROR_CODE {
        return Err(format!(
            "Unknown error: validation returned a variant not mapped to a Go error code (expected {expected_error_code})"
        ));
    }
    if actual_code != expected_error_code {
        return Err(format!(
            "Expected error code {expected_error_code}, got {actual_code}"
        ));
    }
    Ok(())
}
