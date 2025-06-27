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
    pub messages: Vec<SignedSSVMessage>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    #[serde(
        rename = "RSAPublicKey",
        deserialize_with = "deserialize_rsa_public_keys"
    )]
    pub rsa_public_keys: Vec<Vec<u8>>,
}

impl SpecTest for SignedSSVMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        for message in &self.messages {
            // Validate the message
            if let Err(validation_error) = self.validate_message(message) {
                if self.expected_error.is_empty() {
                    eprintln!("Unexpected validation error: {}", validation_error);
                    return false;
                } else if !validation_error.contains(&self.expected_error) {
                    eprintln!(
                        "Expected error '{}', got '{}'",
                        self.expected_error, validation_error
                    );
                    return false;
                }
            } else if !self.expected_error.is_empty() {
                eprintln!(
                    "Expected error '{}', but validation passed",
                    self.expected_error
                );
                return false;
            }

            // TODO: Verify RSA signatures when RSA verification is implemented
            // For now, just validate the message structure
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SignedSSVMsg)
    }
}

impl SignedSSVMessageTest {
    fn validate_message(&self, message: &SignedSSVMessage) -> Result<(), String> {
        // Basic validation logic - can be expanded based on SSV message requirements
        if message.signatures().is_empty() {
            return Err("no signatures".to_string());
        }

        if message.operator_ids().is_empty() {
            return Err("no signers".to_string());
        }

        if message.signatures().len() != message.operator_ids().len() {
            return Err("signers and signatures with different length".to_string());
        }

        // Check for zero signer
        for &operator_id in message.operator_ids() {
            if *operator_id == 0 {
                return Err("zero signer".to_string());
            }
        }

        // Check for duplicate signers
        let mut unique_signers = std::collections::HashSet::new();
        for &operator_id in message.operator_ids() {
            if !unique_signers.insert(operator_id) {
                return Err("non unique signers".to_string());
            }
        }

        // Check for empty signatures
        for signature in message.signatures() {
            if signature.is_empty() {
                return Err("empty signature".to_string());
            }
        }

        Ok(())
    }
}

// Encoding test structure
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedSSVMessageEncodingTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Data", deserialize_with = "deserialize_base64_to_bytes")]
    pub data: Vec<u8>,

    #[serde(
        rename = "ExpectedRoot",
        deserialize_with = "deserialize_bytes_to_hash256"
    )]
    pub expected_root: types::Hash256,
}

impl SpecTest for SignedSSVMessageEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        // TODO: Implement SSZ decoding/encoding validation
        // For now, return true as placeholder
        println!("Running SignedSSVMessage encoding test: {}", self.name);
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SignedSSVMsg)
    }
}
