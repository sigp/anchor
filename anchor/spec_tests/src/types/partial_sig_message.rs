use message_validator::validate_partial_signature_message_basic_semantics;
use serde::Deserialize;
use ssv_types::partial_sig::PartialSignatureMessages;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};

// Partial signature message test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialSigMsgSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Messages")]
    #[serde(deserialize_with = "deserialize_partial_signature_messages")]
    pub messages: Vec<Result<PartialSignatureMessages, String>>,
    #[serde(rename = "EncodedMessages")]
    pub encoded_messages: Option<Vec<Vec<u8>>>,
    #[serde(rename = "ExpectedRoots")]
    pub expected_roots: Option<Vec<[u8; 32]>>,
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
        let has_expected_error = !self.expected_error.is_empty();

        for message_result in &self.messages {
            match message_result {
                Ok(partial_sig_messages) => {
                    // We successfully parsed the message, now validate using actual client logic
                    match validate_partial_signature_message_basic_semantics(partial_sig_messages) {
                        Ok(()) => {
                            // Validation passed - this should only happen if no error is expected
                            if has_expected_error {
                                println!(
                                    "❌ Validation passed but expected error: {}",
                                    self.expected_error
                                );
                                return false;
                            } else {
                                println!(
                                    "✅ PartialSignatureMessage validation passed: {}",
                                    self.name
                                );
                            }
                        }
                        Err(e) => {
                            // Validation failed using client validation - check if this matches expected error
                            let validation_error = format!("{:?}", e);
                            if has_expected_error {
                                if self.is_matching_error(&validation_error, &self.expected_error) {
                                    println!(
                                        "✅ Got expected error '{}' for test: {}",
                                        validation_error, self.name
                                    );
                                    return true; // Expected error occurred
                                } else {
                                    println!(
                                        "❌ Got error '{}' but expected '{}' for test: {}",
                                        validation_error, self.expected_error, self.name
                                    );
                                    return false;
                                }
                            } else {
                                println!(
                                    "❌ Unexpected validation error '{}' for test: {}",
                                    validation_error, self.name
                                );
                                return false;
                            }
                        }
                    }
                }
                Err(parse_error) => {
                    // Failed to parse the message from JSON - this is expected for some tests
                    if has_expected_error {
                        if self.is_matching_error(parse_error, &self.expected_error) {
                            println!(
                                "✅ Got expected parse error '{}' for test: {}",
                                parse_error, self.name
                            );
                            return true;
                        } else {
                            println!(
                                "❌ Got parse error '{}' but expected '{}' for test: {}",
                                parse_error, self.expected_error, self.name
                            );
                            return false;
                        }
                    } else {
                        println!(
                            "❌ Unexpected parse error '{}' for test: {}",
                            parse_error, self.name
                        );
                        return false;
                    }
                }
            }
        }

        // If we get here, all messages validated successfully and no error was expected
        !has_expected_error
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::PartialSigMessage)
    }
}

impl PartialSigMsgSpecTest {
    /// Match client error messages to expected Go spec error messages
    fn is_matching_error(&self, actual_error: &str, expected_error: &str) -> bool {
        // Direct match
        if actual_error.contains(expected_error) || expected_error.contains(actual_error) {
            return true;
        }

        // Map client errors to Go spec expected errors
        match expected_error {
            "no PartialSignatureMessages messages" => {
                actual_error.contains("NoPartialSignatureMessages")
            }
            "inconsistent signers" => actual_error.contains("InconsistentSigners"),
            "message invalid: signer ID 0 not allowed" => actual_error.contains("ZeroSigner"),
            "too many signatures" => actual_error.contains("TooManyPartialSignatureMessages"),
            "invalid signature kind" => {
                actual_error.contains("InvalidPartialSignatureType")
                    || actual_error.contains("PartialSignatureTypeRoleMismatch")
            }
            _ => false,
        }
    }
}
