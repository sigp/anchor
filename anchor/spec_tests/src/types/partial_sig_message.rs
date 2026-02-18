use serde::Deserialize;
use ssv_types::{OperatorId, ValidatorIndex, partial_sig::PartialSignatureKind};
use ssz::{Decode, Encode};
use ssz_types::VariableList;
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    SpecTest,
    utils::deserializers::{deserialize_hash256_list_option, deserialize_hex_option},
};

/// Go error codes from ssv-spec `types/error.go` relevant to PartialSignatureMessages validation.
mod error_codes {
    pub const NO_ERROR: i64 = 0;
    pub const ZERO_SIGNER_NOT_ALLOWED: i64 = 17;
    pub const INCONSISTENT_SIGNERS: i64 = 18;
    pub const NO_PARTIAL_SIG_MESSAGES: i64 = 19;
}

/// Intermediate struct for deserializing individual partial signature messages from JSON.
///
/// Still needed because validation-error fixtures contain invalid data (e.g. empty hex for
/// `PartialSignature`) that cannot deserialize into `bls::Signature`. The Anchor types
/// now have `Deserialize` behind the `serde` feature for use with valid data (e.g.
/// `StructureSizeTest`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TestPartialSignatureMessage {
    #[serde(deserialize_with = "deserialize_hex_option", default)]
    partial_signature: Option<Vec<u8>>,
    signer: OperatorId,
    #[serde(deserialize_with = "deserialize_hex_option", default)]
    signing_root: Option<Vec<u8>>,
    #[serde(default)]
    validator_index: Option<String>,
}

/// Intermediate struct for deserializing `PartialSignatureMessages` from JSON.
///
/// Kept because the inner `messages` field contains `TestPartialSignatureMessage` shims
/// (see above). `PartialSignatureKind` now deserializes directly from u64 via the `serde`
/// feature.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TestPartialSignatureMessages {
    #[serde(
        rename = "Type",
        deserialize_with = "ssv_types::deserializers::deserialize_partial_signature_kind"
    )]
    kind: PartialSignatureKind,
    slot: String,
    messages: Vec<TestPartialSignatureMessage>,
}

/// Top-level test fixture for `MsgSpecTest`.
///
/// Uses local validation (Anchor's `PartialSignatureMessages` has no `validate()` method —
/// validation lives in `message_validator` in production). Maps validation errors to Go
/// integer error codes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PartialSigMsgSpecTest {
    messages: Vec<TestPartialSignatureMessages>,
    #[serde(default)]
    encoded_messages: Option<Vec<String>>,
    #[serde(deserialize_with = "deserialize_hash256_list_option", default)]
    expected_roots: Option<Vec<Hash256>>,
    expected_error_code: i64,
}

impl SpecTest for PartialSigMsgSpecTest {
    fn run(&self) -> Result<(), String> {
        let mut last_error_code: i64 = error_codes::NO_ERROR;

        for (i, msg) in self.messages.iter().enumerate() {
            // Validate the message (local validation — mirrors Go's msg.Validate())
            match Self::validate_message(msg) {
                Ok(()) => {}
                Err(code) => last_error_code = code,
            }

            // Test encoding/decoding if encoded messages are provided
            if let Some(ref encoded_messages) = self.encoded_messages {
                let anchor_msg = Self::build_anchor_message(msg)
                    .map_err(|e| format!("Failed to build message {i}: {e}"))?;

                let encoded_bytes = Self::base64_decode(&encoded_messages[i])
                    .map_err(|e| format!("Failed to decode base64 for message {i}: {e}"))?;

                let actual_encoded = anchor_msg.as_ssz_bytes();
                if actual_encoded != encoded_bytes {
                    return Err(format!(
                        "Encoding mismatch for message {i}: expected {} bytes, got {} bytes",
                        encoded_bytes.len(),
                        actual_encoded.len(),
                    ));
                }

                let decoded = ssv_types::partial_sig::PartialSignatureMessages::from_ssz_bytes(
                    &actual_encoded,
                )
                .map_err(|e| format!("Roundtrip decode failed for message {i}: {e:?}"))?;

                if decoded.tree_hash_root() != anchor_msg.tree_hash_root() {
                    return Err(format!("Root mismatch after roundtrip for message {i}"));
                }
            }

            // Check expected roots if provided
            if let Some(ref expected_roots) = self.expected_roots {
                let anchor_msg = Self::build_anchor_message(msg)
                    .map_err(|e| format!("Failed to build message {i}: {e}"))?;

                let root = anchor_msg.tree_hash_root();
                if root != expected_roots[i] {
                    return Err(format!(
                        "Root mismatch for message {i}: expected {}, got {root}",
                        expected_roots[i],
                    ));
                }
            }
        }

        if last_error_code != self.expected_error_code {
            return Err(format!(
                "Expected error code {}, got {last_error_code}",
                self.expected_error_code,
            ));
        }

        Ok(())
    }
}

impl PartialSigMsgSpecTest {
    /// Local validation — mirrors Go's `PartialSignatureMessages.Validate()`.
    ///
    /// Anchor's `PartialSignatureMessages` has no `validate()` method (validation lives in
    /// `message_validator`), so we implement the validation checks locally.
    ///
    /// Go's validation order: empty check → for each message: inconsistent signer → m.Validate()
    /// where m.Validate() checks signer ID 0. We must match this order for error code parity.
    fn validate_message(msg: &TestPartialSignatureMessages) -> Result<(), i64> {
        if msg.messages.is_empty() {
            return Err(error_codes::NO_PARTIAL_SIG_MESSAGES);
        }

        let first_signer = msg.messages[0].signer;

        for m in &msg.messages {
            // Check signer consistency first (Go checks this before m.Validate())
            if first_signer != m.signer {
                return Err(error_codes::INCONSISTENT_SIGNERS);
            }

            // Then check signer ID 0 (Go's m.Validate())
            if m.signer == OperatorId(0) {
                return Err(error_codes::ZERO_SIGNER_NOT_ALLOWED);
            }
        }

        Ok(())
    }

    /// Build an Anchor `PartialSignatureMessages` from the test intermediate struct.
    ///
    /// Only used for encoding/root tests where we need the actual Anchor type.
    fn build_anchor_message(
        msg: &TestPartialSignatureMessages,
    ) -> Result<ssv_types::partial_sig::PartialSignatureMessages, String> {
        let slot = msg
            .slot
            .parse::<u64>()
            .map_err(|e| format!("Invalid slot: {e}"))?;

        let mut messages = Vec::new();
        for m in &msg.messages {
            let sig_bytes = m
                .partial_signature
                .as_ref()
                .ok_or("Missing partial_signature")?;

            let partial_signature = bls::Signature::deserialize(sig_bytes)
                .map_err(|e| format!("Invalid BLS signature: {e:?}"))?;

            let root_bytes = m.signing_root.as_ref().ok_or("Missing signing_root")?;
            let signing_root = if root_bytes.len() == 32 {
                Hash256::from_slice(root_bytes)
            } else {
                return Err(format!(
                    "Invalid signing_root length: expected 32, got {}",
                    root_bytes.len()
                ));
            };

            let validator_index = m
                .validator_index
                .as_ref()
                .ok_or("Missing validator_index")?
                .parse::<usize>()
                .map_err(|e| format!("Invalid validator_index: {e}"))?;

            messages.push(ssv_types::partial_sig::PartialSignatureMessage {
                partial_signature,
                signing_root,
                signer: m.signer,
                validator_index: ValidatorIndex(validator_index),
            });
        }

        Ok(ssv_types::partial_sig::PartialSignatureMessages {
            kind: msg.kind,
            slot: types::Slot::new(slot),
            messages: VariableList::new(messages)
                .map_err(|_| "Too many partial signature messages".to_string())?,
        })
    }

    fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        STANDARD
            .decode(s)
            .map_err(|e| format!("base64 decode error: {e}"))
    }
}
