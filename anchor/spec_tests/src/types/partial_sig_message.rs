use serde::Deserialize;
use ssv_types::partial_sig::PartialSignatureMessages;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    SpecTest, SpecTestType,
    types::TypesSpecTestType,
    utils::deserializers::{deserialize_base64_list_option, deserialize_hash256_list_option},
};

// Partial signature message test
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct PartialSigMsgSpecTest {
    #[serde(rename = "Type")]
    pub r#type: String,
    pub documentation: String,
    pub name: String,
    pub messages: Vec<PartialSignatureMessages>,
    #[serde(deserialize_with = "deserialize_base64_list_option", default)]
    pub encoded_messages: Option<Vec<Vec<u8>>>,
    #[serde(deserialize_with = "deserialize_hash256_list_option", default)]
    pub expected_roots: Option<Vec<Hash256>>,
    pub expected_error: String,
}

impl SpecTest for PartialSigMsgSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        let mut last_error: Option<String> = None;

        for (i, msg) in self.messages.iter().enumerate() {
            if let Err(err) = validate_message(msg) {
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
            if let Some(ref expected_roots) = self.expected_roots
                && msg.tree_hash_root() != expected_roots[i]
            {
                return false;
            }
        }

        if !self.expected_error.is_empty() {
            // We do not have an error when we expected one
            match last_error {
                Some(error) => self.expected_error == error,
                None => false,
            }
        } else {
            // If we do do not have an expected error, then last_error should be None.
            last_error.is_none()
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::PartialSigMessage)
    }
}

// Handeled by message validator
fn validate_message(msg: &PartialSignatureMessages) -> Result<(), String> {
    if msg.messages.is_empty() {
        return Err("no PartialSignatureMessages messages".to_string());
    }

    let signer = msg.messages[0].signer;
    // Validate each message and check consistency
    for message in &msg.messages {
        // Check signer consistency
        if message.signer != signer {
            return Err("inconsistent signers".to_string());
        }

        // Signer ID 0 is not allowed
        if message.signer.0 == 0 {
            return Err("message invalid: signer ID 0 not allowed".to_string());
        }
    }

    Ok(())
}
