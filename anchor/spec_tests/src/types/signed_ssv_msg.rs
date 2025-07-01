use serde::Deserialize;
use ssv_types::message::SignedSSVMessage;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};

// SignedSSVMessage validation tests
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedSSVMessageTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Messages")]
    #[serde(deserialize_with = "deserialize_signed_ssv_messages")]
    pub messages: Vec<Result<SignedSSVMessage, String>>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    #[serde(rename = "RSAPublicKey")]
    pub rsa_public_keys: Option<Vec<String>>,
}

impl SpecTest for SignedSSVMessageTest {
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
                Ok(signed_message) => {
                    // We successfully parsed the message, now validate it using client logic
                    match signed_message.validate() {
                        Ok(()) => {
                            // Validation passed - this should only happen if no error is expected
                            if has_expected_error {
                                println!(
                                    "❌ Validation passed but expected error: {}",
                                    self.expected_error
                                );
                                return false;
                            } else {
                                println!("✅ Message validation passed: {}", self.name);
                            }
                        }
                        Err(e) => {
                            // Validation failed using client validation - check if this matches expected error
                            let validation_error = format!("{}", e);
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
        SpecTestType::Types(TypesSpecTestType::SignedSSVMsg)
    }
}

impl SignedSSVMessageTest {
    /// Match client error messages to expected Go spec error messages
    fn is_matching_error(&self, actual_error: &str, expected_error: &str) -> bool {
        // Direct match
        if actual_error.contains(expected_error) || expected_error.contains(actual_error) {
            return true;
        }

        // Map client errors to Go spec expected errors
        match expected_error {
            "no signers" => actual_error.contains("No signers were provided"),
            "no signatures" => actual_error.contains("No signatures provided"),
            "non unique signer" => actual_error.contains("A duplicated signer was found"),
            "number of signatures is different than number of signers" => {
                actual_error.contains("Signers and signatures must have the same length")
            }
            "signers not sorted" => actual_error.contains("Signers are not sorted"),
            "too many signatures" => actual_error.contains("Too many signatures"),
            "too many operator IDs" => actual_error.contains("Too many operator IDs"),
            "zero signer" | "signer ID 0 not allowed" => {
                actual_error.contains("signer ID 0 not allowed")
                    || actual_error.contains("Zero signer")
            }
            "empty signature" => actual_error.contains("empty signature"),
            "nil SSVMessage" => actual_error.contains("nil SSVMessage"),
            _ => false,
        }
    }
}
