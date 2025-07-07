use base64::prelude::*;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Verifier};
use operator_key::public;
use serde::Deserialize;
use ssv_types::{
    OperatorId,
    message::{SSVMessage, SignedSSVMessage, SignedSSVMessageError},
};
use ssz::Encode;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};

// Intermediate test-specific SignedSSVMessage that can handle null SSVMessage
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestSignedSSVMessage {
    #[serde(rename = "Signatures")]
    pub signatures: Vec<String>,
    #[serde(rename = "OperatorIDs")]
    pub operator_ids: Vec<OperatorId>,
    #[serde(rename = "SSVMessage")]
    pub ssv_message: Option<SSVMessage>,
    #[serde(rename = "FullData")]
    pub full_data: Option<String>,
}

// SignedSSVMessage validation tests
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedSSVMessageTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Messages")]
    pub messages: Vec<TestSignedSSVMessage>,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
    #[serde(rename = "RSAPublicKey")]
    pub rsa_public_key: Option<Vec<String>>,
}

impl SpecTest for SignedSSVMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        // go through all of the messages
        for test_msg in &self.messages {
            // Handle null SSVMessage case
            let ssv_message = match &test_msg.ssv_message {
                Some(msg) => msg,
                None => return self.check_expected_error("nil SSVMessage"),
            };

            // Now, we can convert it to an actual SignedSSVMessage for validation
            let signed_msg = match self.convert_test_message(test_msg, ssv_message) {
                Ok(msg) => msg,
                Err(error) => {
                    // Check if we ran into an expected error
                    return error == self.expected_error;
                }
            };

            // Encode the ssv message
            let encoded_ssv_msg = match signed_msg.validate() {
                Ok(_) => signed_msg.ssv_message().as_ssz_bytes(),
                Err(_) => return false,
            };

            // Now, we need to verify the RSA signatures
            if let Some(ref pk_strings) = self.rsa_public_key {
                for (i, pk_string) in pk_strings.iter().enumerate() {
                    let rsa_key = match public::from_base64(pk_string.as_bytes()) {
                        Ok(key) => key,
                        Err(_) => return false,
                    };

                    // Convert to PKey for verification
                    let pkey = match PKey::from_rsa(rsa_key) {
                        Ok(key) => key,
                        Err(_) => return false,
                    };

                    // Verify signature using PKCS1v15 padding with SHA256
                    let mut verifier = match Verifier::new(MessageDigest::sha256(), &pkey) {
                        Ok(v) => v,
                        Err(_) => return false,
                    };

                    if verifier.update(&encoded_ssv_msg).is_err() {
                        return false;
                    }

                    let signature: &[u8] = &signed_msg.signatures()[i];
                    if verifier.verify(signature).is_err() {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SignedSSVMsg)
    }
}

impl SignedSSVMessageTest {
    fn convert_test_message(
        &self,
        test_msg: &TestSignedSSVMessage,
        ssv_message: &SSVMessage,
    ) -> Result<SignedSSVMessage, String> {
        // Convert base64 signatures to byte arrays
        let mut signatures = Vec::new();

        // Most of the signatures we are given are too short, so we have to pad them to a valid
        // length
        for sig_str in &test_msg.signatures {
            if sig_str.is_empty() {
                return Err("empty signature".to_string());
            }
            let sig_bytes = BASE64_STANDARD
                .decode(sig_str.as_bytes())
                .map_err(|_| "failed to decode base64 signature")?;

            // Pad or truncate signature to 256 bytes for RSA signature format
            let mut sig_array = [0u8; 256];
            if sig_bytes.len() <= 256 {
                sig_array[..sig_bytes.len()].copy_from_slice(&sig_bytes);
            } else {
                sig_array.copy_from_slice(&sig_bytes[..256]);
            }
            signatures.push(sig_array);
        }

        // Create our SignedSSVMessage
        SignedSSVMessage::new_from_vecs(
            signatures,
            test_msg.operator_ids.clone(),
            ssv_message.clone(),
            Vec::new(),
        )
        .map_err(|e| self.error_to_string(&e))
    }

    fn error_to_string(&self, error: &SignedSSVMessageError) -> String {
        match error {
            SignedSSVMessageError::NoSigners => "no signers".to_string(),
            SignedSSVMessageError::ZeroSigner => "signer ID 0 not allowed".to_string(),
            SignedSSVMessageError::DuplicatedSigner => "non unique signer".to_string(),
            SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => {
                "number of signatures is different than number of signers".to_string()
            }
            SignedSSVMessageError::NoSignatures => "no signatures".to_string(),
            _ => "invalid error".to_string(),
        }
    }

    fn check_expected_error(&self, error_msg: &str) -> bool {
        !self.expected_error.is_empty() && self.expected_error == error_msg
    }
}
