//! Go error codes from ssv-spec `types/error.go` (iota + 1).
//!
//! Only codes relevant to currently implemented spec tests are included.
//! Add new codes as new test types are implemented.

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

// --- Sentinel ---

/// Sentinel for Anchor-specific errors without Go equivalents.
pub const UNMAPPED_ERROR_CODE: i64 = -1;
