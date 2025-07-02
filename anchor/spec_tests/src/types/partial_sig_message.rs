use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;
use ssv_types::partial_sig::{PartialSignatureError, PartialSignatureMessages};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

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
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running test: {}", self.name);
        let mut last_error: Option<PartialSignatureError> = None;

        for (i, msg) in self.messages.iter().enumerate() {
            // Test validation
            if let Err(err) = msg.validate() {
                last_error = Some(err);
            }

            // Test encoding/decoding if we have encoded messages
            if let Some(ref encoded_messages) = self.encoded_messages {
                if i < encoded_messages.len() {
                    // Test encoding
                    let encoded = msg.as_ssz_bytes();
                    if encoded != encoded_messages[i] {
                        println!("Test '{}' encoding mismatch at index {}", self.name, i);
                        return false;
                    }

                    // Test decoding
                    let decoded = match PartialSignatureMessages::from_ssz_bytes(&encoded) {
                        Ok(decoded) => decoded,
                        Err(e) => {
                            println!(
                                "Test '{}' failed to decode at index {}: {:?}",
                                self.name, i, e
                            );
                            return false;
                        }
                    };

                    // Verify decoded matches original
                    if decoded != *msg {
                        println!(
                            "Test '{}' roundtrip encoding failed at index {}",
                            self.name, i
                        );
                        return false;
                    }

                    // Verify tree hash roots match
                    let decoded_root = decoded.tree_hash_root();
                    let original_root = msg.tree_hash_root();
                    if decoded_root != original_root {
                        println!(
                            "Test '{}' tree hash mismatch after roundtrip at index {}",
                            self.name, i
                        );
                        return false;
                    }
                }
            }

            // Test expected roots if provided
            if let Some(ref expected_roots) = self.expected_roots {
                if i < expected_roots.len() {
                    let computed_root = msg.tree_hash_root();
                    if computed_root != expected_roots[i] {
                        println!(
                            "Test '{}' expected root mismatch at index {}. Expected: {:?}, Got: {:?}",
                            self.name, i, expected_roots[i], computed_root
                        );
                        return false;
                    }
                }
            }
        }

        // Check if we got the expected error
        let result = if self.expected_error.is_empty() {
            // No error expected
            if let Some(err) = last_error {
                println!("Test '{}' got unexpected error: {}", self.name, err);
                false
            } else {
                true
            }
        } else {
            // Error expected
            if let Some(err) = last_error {
                let error_matches = self.check_error_message(&err);
                if !error_matches {
                    println!(
                        "Test '{}' error mismatch. Expected: '{}', Got: '{}'",
                        self.name, self.expected_error, err
                    );
                }
                error_matches
            } else {
                println!(
                    "Test '{}' expected error '{}' but got none",
                    self.name, self.expected_error
                );
                false
            }
        };

        println!("Test '{}' result: {}", self.name, result);
        result
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::PartialSigMessage)
    }
}

impl PartialSigMsgSpecTest {
    /// Check if the error message matches the expected error from Go tests
    fn check_error_message(&self, error: &PartialSignatureError) -> bool {
        // Map Rust errors to Go error messages
        let go_error = match error {
            PartialSignatureError::NoMessages => "no PartialSignatureMessages messages",
            PartialSignatureError::InconsistentSigners => "inconsistent signers",
            PartialSignatureError::ZeroSigner => "message invalid: signer ID 0 not allowed",
        };

        self.expected_error == go_error
    }
}
