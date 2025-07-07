use serde::Deserialize;
use ssv_types::partial_sig::{PartialSignatureError, PartialSignatureMessages};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType, utils::deserializers::type_parse::*,
};

// Partial signature message test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialSigMsgSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Messages")]
    pub messages: Vec<PartialSignatureMessages>,
    #[serde(
        rename = "EncodedMessages",
        deserialize_with = "deserialize_optional_base64_vec",
        default
    )]
    pub encoded_messages: Option<Vec<Vec<u8>>>,
    #[serde(
        rename = "ExpectedRoots",
        deserialize_with = "deserialize_optional_hash256_vec",
        default
    )]
    pub expected_roots: Option<Vec<Hash256>>,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for PartialSigMsgSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        let mut last_error: Option<PartialSignatureError> = None;

        for (i, msg) in self.messages.iter().enumerate() {
            // Test validation
            if let Err(err) = msg.validate() {
                last_error = Some(err);
            }

            // Test encoding/decoding if we have encoded messages
            if let Some(ref encoded_messages) = self.encoded_messages {
                // Test encoding
                let encoded = msg.as_ssz_bytes();
                if encoded != encoded_messages[i] {
                    return false;
                }

                // Test decoding
                let decoded = match PartialSignatureMessages::from_ssz_bytes(&encoded) {
                    Ok(decoded) => decoded,
                    Err(_) => return false,
                };

                // Verify decoded matches original
                if decoded != *msg {
                    return false;
                }

                // Verify tree hash roots match
                if decoded.tree_hash_root() != msg.tree_hash_root() {
                    return false;
                }
            }

            // Test expected roots if provided
            if let Some(ref expected_roots) = self.expected_roots {
                if msg.tree_hash_root() != expected_roots[i] {
                    return false;
                }
            }
        }

        if !self.expected_error.is_empty() {
            // We have an expected error, so last_error should be Some and it should match
            self.check_error_message(&last_error)
        } else {
            // If we do do not have an expected error, then last_error should be None.
            last_error.is_none()
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::PartialSigMessage)
    }
}

impl PartialSigMsgSpecTest {
    /// Check if the error message matches the expected error from Go tests
    fn check_error_message(&self, error: &Option<PartialSignatureError>) -> bool {
        let error = match error {
            Some(error) => error,
            None => return false,
        };

        // Map Rust errors to Go error messages
        let go_error = match error {
            PartialSignatureError::NoMessages => "no PartialSignatureMessages messages",
            PartialSignatureError::InconsistentSigners => "inconsistent signers",
            PartialSignatureError::ZeroSigner => "message invalid: signer ID 0 not allowed",
        };

        self.expected_error == go_error
    }
}
