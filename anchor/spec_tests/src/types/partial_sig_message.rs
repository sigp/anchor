use serde::Deserialize;
use ssv_types::{
    OperatorId, ValidatorIndex,
    partial_sig::{
        PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages,
        PartialSignatureMessagesError,
    },
};
use ssz::{Decode, Encode};
use ssz_types::VariableList;
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    SpecTest,
    utils::{
        decode_base64,
        deserializers::{deserialize_hash256_list_option, deserialize_hex_option},
        error_codes,
    },
};

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
/// Calls `PartialSignatureMessages::validate()` and maps validation errors to Go
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
        // Go's MsgSpecTest checks all messages and keeps the last error code.
        // This differs from `SignedSSVMessageTest` which checks per-message.
        let mut last_error_code: i64 = error_codes::NO_ERROR;

        for (i, msg) in self.messages.iter().enumerate() {
            match Self::validate_message(msg) {
                Ok(()) => {}
                Err(code) => last_error_code = code,
            }

            // Build the Anchor type once if encoding or root checks are needed
            let anchor_msg = if self.encoded_messages.is_some() || self.expected_roots.is_some() {
                Some(
                    Self::build_anchor_message(msg)
                        .map_err(|e| format!("Failed to build message {i}: {e}"))?,
                )
            } else {
                None
            };

            // Test encoding/decoding if encoded messages are provided
            if let Some(ref encoded_messages) = self.encoded_messages {
                let anchor_msg = anchor_msg.as_ref().expect("built above when Some");

                let expected = encoded_messages.get(i).ok_or_else(|| {
                    format!(
                        "Message index {i} out of range for encoded_messages (len {})",
                        encoded_messages.len()
                    )
                })?;
                let encoded_bytes = decode_base64(expected)
                    .map_err(|e| format!("Failed to decode base64 for message {i}: {e}"))?;

                let actual_encoded = anchor_msg.as_ssz_bytes();
                if actual_encoded != encoded_bytes {
                    return Err(format!(
                        "Encoding mismatch for message {i}: expected {} bytes, got {} bytes",
                        encoded_bytes.len(),
                        actual_encoded.len(),
                    ));
                }

                let decoded = PartialSignatureMessages::from_ssz_bytes(&actual_encoded)
                    .map_err(|e| format!("Roundtrip decode failed for message {i}: {e:?}"))?;

                if decoded.tree_hash_root() != anchor_msg.tree_hash_root() {
                    return Err(format!("Root mismatch after roundtrip for message {i}"));
                }
            }

            // Check expected roots if provided
            if let Some(ref expected_roots) = self.expected_roots {
                let anchor_msg = anchor_msg.as_ref().expect("built above when Some");

                let expected_root = expected_roots.get(i).ok_or_else(|| {
                    format!(
                        "Message index {i} out of range for expected_roots (len {})",
                        expected_roots.len()
                    )
                })?;
                let root = anchor_msg.tree_hash_root();
                if root != *expected_root {
                    return Err(format!(
                        "Root mismatch for message {i}: expected {expected_root}, got {root}",
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
    /// Validate using Anchor's production `PartialSignatureMessages::validate()`.
    ///
    /// Builds a production type from the test fixture (with placeholder BLS data,
    /// since validation only inspects signer fields) and calls `validate()`.
    fn validate_message(msg: &TestPartialSignatureMessages) -> Result<(), i64> {
        let messages: Vec<PartialSignatureMessage> = msg
            .messages
            .iter()
            .map(|m| PartialSignatureMessage {
                partial_signature: bls::Signature::empty(),
                signing_root: Hash256::default(),
                signer: m.signer,
                validator_index: ValidatorIndex(0),
            })
            .collect();

        let production_msg = PartialSignatureMessages {
            kind: msg.kind,
            slot: types::Slot::new(0),
            messages: VariableList::new(messages).expect("test fixture within bounds"),
        };

        production_msg.validate().map_err(|e| match e {
            PartialSignatureMessagesError::Empty => error_codes::NO_PARTIAL_SIG_MESSAGES,
            PartialSignatureMessagesError::InconsistentSigners => error_codes::INCONSISTENT_SIGNERS,
            PartialSignatureMessagesError::ZeroSigner => error_codes::ZERO_SIGNER_NOT_ALLOWED,
        })
    }

    /// Build an Anchor `PartialSignatureMessages` from the test intermediate struct.
    ///
    /// Only used for encoding/root tests where we need the actual Anchor type.
    fn build_anchor_message(
        msg: &TestPartialSignatureMessages,
    ) -> Result<PartialSignatureMessages, String> {
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
            if root_bytes.len() != 32 {
                return Err(format!(
                    "Invalid signing_root length: expected 32, got {}",
                    root_bytes.len()
                ));
            }
            let signing_root = Hash256::from_slice(root_bytes);

            let validator_index = m
                .validator_index
                .as_ref()
                .ok_or("Missing validator_index")?
                .parse::<usize>()
                .map_err(|e| format!("Invalid validator_index: {e}"))?;

            messages.push(PartialSignatureMessage {
                partial_signature,
                signing_root,
                signer: m.signer,
                validator_index: ValidatorIndex(validator_index),
            });
        }

        Ok(PartialSignatureMessages {
            kind: msg.kind,
            slot: types::Slot::new(slot),
            messages: VariableList::new(messages)
                .map_err(|_| "Too many partial signature messages".to_string())?,
        })
    }
}
