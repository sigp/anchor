use openssl::{hash::MessageDigest, pkey::PKey, sign::Verifier};
use serde::Deserialize;
use ssv_types::{
    OperatorId,
    message::{SSVMessage, SignedSSVMessage, SignedSSVMessageError},
};
use ssz::Encode;

use crate::{SpecTest, utils::deserializers::deserialize_base64_list};

/// Go error codes from ssv-spec `types/error.go` (iota + 1).
/// Only those relevant to `SignedSSVMessage.Validate()` and RSA verification.
mod error_codes {
    pub const NO_ERROR: i64 = 0;
    pub const NON_UNIQUE_SIGNER: i64 = 9;
    pub const INCORRECT_NUMBER_OF_SIGNATURES: i64 = 12;
    pub const EMPTY_SIGNATURE: i64 = 13;
    pub const NIL_SSV_MESSAGE: i64 = 14;
    pub const NO_SIGNATURES: i64 = 15;
    pub const NO_SIGNERS: i64 = 16;
    pub const ZERO_SIGNER_NOT_ALLOWED: i64 = 17;
    pub const SSV_MESSAGE_HAS_INVALID_SIGNATURE: i64 = 46;
    /// Sentinel for Anchor-specific errors without Go equivalents.
    pub const UNMAPPED_ERROR_CODE: i64 = -1;
}

/// Intermediate struct for a single signed message from the fixture.
///
/// `SSVMessage` is `Option` because Go uses a pointer (can be null in error fixtures).
/// `SSVMessage` deserialization is handled by the feature-gated serde support in `ssv_types`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TestSignedSSVMessage {
    #[serde(deserialize_with = "deserialize_base64_list")]
    signatures: Vec<Vec<u8>>,
    #[serde(rename = "OperatorIDs")]
    operator_ids: Vec<OperatorId>,
    #[serde(rename = "SSVMessage")]
    ssv_message: Option<SSVMessage>,
    #[serde(
        rename = "FullData",
        deserialize_with = "crate::utils::deserializers::deserialize_hex_option",
        default
    )]
    full_data: Option<Vec<u8>>,
}

/// Top-level test fixture for `SignedSSVMessageTest`.
///
/// Uses Anchor's `SignedSSVMessage::new()` + `validate()` to exercise actual
/// validation code, then maps `SignedSSVMessageError` variants to Go error codes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SignedSSVMessageTest {
    messages: Vec<TestSignedSSVMessage>,
    expected_error_code: i64,
    #[serde(rename = "RSAPublicKey", default)]
    rsa_public_key: Option<Vec<String>>,
}

impl SpecTest for SignedSSVMessageTest {
    fn run(&self) -> Result<(), String> {
        for msg in &self.messages {
            let actual_code = match self.validate_message(msg) {
                Ok(()) => error_codes::NO_ERROR,
                Err(code) => code,
            };

            if actual_code != self.expected_error_code {
                return Err(format!(
                    "Expected error code {}, got {actual_code}",
                    self.expected_error_code,
                ));
            }
        }
        Ok(())
    }
}

impl SignedSSVMessageTest {
    /// Validate a signed SSV message using Anchor's actual validation code.
    ///
    /// Two pre-checks are needed before calling `SignedSSVMessage::new()`:
    /// 1. Empty signatures — `new()` requires `[u8; 256]` per signature
    /// 2. Null SSVMessage — `new()` requires an `SSVMessage` parameter
    ///
    /// After construction, `new()` calls `validate()` internally, catching all
    /// other validation errors which we map to Go error codes.
    fn validate_message(&self, msg: &TestSignedSSVMessage) -> Result<(), i64> {
        // Pre-check: empty signatures (Anchor requires [u8; 256], can't represent 0 bytes)
        for sig in &msg.signatures {
            if sig.is_empty() {
                return Err(error_codes::EMPTY_SIGNATURE);
            }
        }

        // Pre-check: null SSVMessage (Anchor's new() requires an SSVMessage parameter)
        let ssv_message = msg
            .ssv_message
            .as_ref()
            .ok_or(error_codes::NIL_SSV_MESSAGE)?;

        // Pad signatures to [u8; 256] for Anchor's type requirement
        let signatures = Self::prepare_signatures(&msg.signatures);

        // Use Anchor's actual validation via SignedSSVMessage::new()
        let signed_msg = SignedSSVMessage::new(
            signatures,
            msg.operator_ids.clone(),
            ssv_message.clone(),
            msg.full_data.clone().unwrap_or_default(),
        )
        .map_err(|e| Self::error_code_for(&e))?;

        // Verify RSA signatures if public keys are provided
        self.verify_rsa_signatures(&signed_msg, ssv_message)
    }

    /// Pad variable-length signatures to `[u8; 256]` arrays for Anchor's type requirement.
    fn prepare_signatures(signatures: &[Vec<u8>]) -> Vec<[u8; 256]> {
        signatures
            .iter()
            .map(|sig| {
                let mut arr = [0u8; 256];
                let len = sig.len().min(256);
                arr[..len].copy_from_slice(&sig[..len]);
                arr
            })
            .collect()
    }

    /// Verify RSA signatures against the SSZ-encoded SSVMessage.
    fn verify_rsa_signatures(
        &self,
        signed_msg: &SignedSSVMessage,
        ssv_message: &SSVMessage,
    ) -> Result<(), i64> {
        let Some(ref pk_strings) = self.rsa_public_key else {
            return Ok(());
        };

        let encoded_msg = ssv_message.as_ssz_bytes();

        for (i, pk_b64) in pk_strings.iter().enumerate() {
            let rsa_key = operator_key::public::from_base64(pk_b64.as_bytes())
                .map_err(|_| error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE)?;

            let pkey = PKey::from_rsa(rsa_key)
                .map_err(|_| error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE)?;

            let mut verifier = Verifier::new(MessageDigest::sha256(), &pkey)
                .map_err(|_| error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE)?;

            verifier
                .update(&encoded_msg)
                .map_err(|_| error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE)?;

            let sig: &[u8] = &signed_msg.signatures()[i];
            let valid = verifier
                .verify(sig)
                .map_err(|_| error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE)?;

            if !valid {
                return Err(error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE);
            }
        }

        Ok(())
    }

    /// Map Anchor's `SignedSSVMessageError` variants to Go's integer error codes.
    fn error_code_for(error: &SignedSSVMessageError) -> i64 {
        match error {
            SignedSSVMessageError::NoSigners => error_codes::NO_SIGNERS,
            SignedSSVMessageError::NoSignatures => error_codes::NO_SIGNATURES,
            SignedSSVMessageError::ZeroSigner => error_codes::ZERO_SIGNER_NOT_ALLOWED,
            SignedSSVMessageError::DuplicatedSigner => error_codes::NON_UNIQUE_SIGNER,
            SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => {
                error_codes::INCORRECT_NUMBER_OF_SIGNATURES
            }
            SignedSSVMessageError::WrongRSASignatureSize { .. } => error_codes::EMPTY_SIGNATURE,
            SignedSSVMessageError::TooManySignatures { .. }
            | SignedSSVMessageError::TooManyOperatorIDs { .. }
            | SignedSSVMessageError::FullDataTooLong { .. }
            | SignedSSVMessageError::SignersNotSorted
            | SignedSSVMessageError::SSVMessageError(_) => {
                // These don't have direct Go error code equivalents in the current fixtures.
                // Using a sentinel value — if a fixture hits this, the test will fail with
                // a clear mismatch, surfacing the discrepancy.
                error_codes::UNMAPPED_ERROR_CODE
            }
        }
    }
}
