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

/// Test fixture shim -> needed because error fixtures contain invalid BLS data that can't
/// deserialize into `bls::Signature`.
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

impl TryFrom<&TestPartialSignatureMessage> for PartialSignatureMessage {
    type Error = String;

    fn try_from(m: &TestPartialSignatureMessage) -> Result<Self, String> {
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

        Ok(PartialSignatureMessage {
            partial_signature,
            signing_root,
            signer: m.signer,
            validator_index: ValidatorIndex(validator_index),
        })
    }
}

/// Test fixture shim — wraps `TestPartialSignatureMessage` shims.
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
        self.check_validation()?;
        self.check_encoding()?;
        self.check_roots()?;
        Ok(())
    }
}

impl PartialSigMsgSpecTest {
    /// Mirrors Go's `MsgSpecTest`: checks all messages, keeps last error code.
    fn check_validation(&self) -> Result<(), String> {
        let mut last_error_code: i64 = error_codes::NO_ERROR;

        for msg in &self.messages {
            if let Err(code) = Self::validate_message(msg) {
                last_error_code = code;
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

    /// Test SSZ encoding roundtrip if `encoded_messages` is provided.
    fn check_encoding(&self) -> Result<(), String> {
        let Some(ref encoded_messages) = self.encoded_messages else {
            return Ok(());
        };

        for (i, msg) in self.messages.iter().enumerate() {
            let partial_sig_msgs = Self::build_message(msg)
                .map_err(|e| format!("Failed to build message {i}: {e}"))?;

            let expected = encoded_messages.get(i).ok_or_else(|| {
                format!(
                    "Message index {i} out of range for encoded_messages (len {})",
                    encoded_messages.len()
                )
            })?;
            let encoded_bytes = decode_base64(expected)
                .map_err(|e| format!("Failed to decode base64 for message {i}: {e}"))?;

            let actual_encoded = partial_sig_msgs.as_ssz_bytes();
            if actual_encoded != encoded_bytes {
                return Err(format!(
                    "Encoding mismatch for message {i}: expected {} bytes, got {} bytes",
                    encoded_bytes.len(),
                    actual_encoded.len(),
                ));
            }

            let decoded = PartialSignatureMessages::from_ssz_bytes(&actual_encoded)
                .map_err(|e| format!("Roundtrip decode failed for message {i}: {e:?}"))?;

            if decoded.tree_hash_root() != partial_sig_msgs.tree_hash_root() {
                return Err(format!("Root mismatch after roundtrip for message {i}"));
            }
        }

        Ok(())
    }

    /// Check expected hash tree roots if `expected_roots` is provided.
    fn check_roots(&self) -> Result<(), String> {
        let Some(ref expected_roots) = self.expected_roots else {
            return Ok(());
        };

        for (i, msg) in self.messages.iter().enumerate() {
            let partial_sig_msgs = Self::build_message(msg)
                .map_err(|e| format!("Failed to build message {i}: {e}"))?;

            let expected_root = expected_roots.get(i).ok_or_else(|| {
                format!(
                    "Message index {i} out of range for expected_roots (len {})",
                    expected_roots.len()
                )
            })?;
            let root = partial_sig_msgs.tree_hash_root();
            if root != *expected_root {
                return Err(format!(
                    "Root mismatch for message {i}: expected {expected_root}, got {root}",
                ));
            }
        }

        Ok(())
    }

    /// Validate via `PartialSignatureMessages::validate()` with placeholder BLS data.
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

        let partial_sig_msgs = PartialSignatureMessages {
            kind: msg.kind,
            slot: types::Slot::new(0),
            messages: VariableList::new(messages).expect("test fixture within bounds"),
        };

        partial_sig_msgs.validate().map_err(|e| match e {
            PartialSignatureMessagesError::Empty => error_codes::NO_PARTIAL_SIG_MESSAGES,
            PartialSignatureMessagesError::InconsistentSigners => error_codes::INCONSISTENT_SIGNERS,
            PartialSignatureMessagesError::ZeroSigner => error_codes::ZERO_SIGNER_NOT_ALLOWED,
        })
    }

    /// Build a `PartialSignatureMessages` from the test fixture (for encoding/root tests).
    fn build_message(
        msg: &TestPartialSignatureMessages,
    ) -> Result<PartialSignatureMessages, String> {
        let slot = msg
            .slot
            .parse::<u64>()
            .map_err(|e| format!("Invalid slot: {e}"))?;

        let messages: Vec<PartialSignatureMessage> = msg
            .messages
            .iter()
            .map(PartialSignatureMessage::try_from)
            .collect::<Result<_, _>>()?;

        Ok(PartialSignatureMessages {
            kind: msg.kind,
            slot: types::Slot::new(slot),
            messages: VariableList::new(messages)
                .map_err(|_| "Too many partial signature messages".to_string())?,
        })
    }
}
