use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use base64::prelude::*;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Verifier};
use operator_key::public;
use serde::Deserialize;
use ssv_types::{
    OperatorId,
    message::{SSVMessage, SignedSSVMessage},
};
use ssz::Encode;

// Test-specific SignedSSVMessage that can handle null SSVMessage
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestSignedSSVMessage {
    #[serde(rename = "Signatures")]
    pub signatures: Vec<String>, // Base64 encoded signatures
    #[serde(rename = "OperatorIDs")]
    pub operator_ids: Vec<OperatorId>,
    #[serde(rename = "SSVMessage")]
    pub ssv_message: Option<SSVMessage>,
    #[serde(rename = "FullData")]
    pub full_data: Option<String>, // Base64 encoded or null
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
    pub rsa_public_key: Option<Vec<String>>, // Base64 encoded PEM keys
}

impl SpecTest for SignedSSVMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        for test_msg in &self.messages {
            // Handle null SSVMessage case
            let ssv_message = match &test_msg.ssv_message {
                Some(msg) => msg,
                None => return self.check_expected_error("nil SSVMessage"),
            };

            // Convert test message to actual SignedSSVMessage for validation
            let signed_msg = match self.convert_test_message(test_msg, ssv_message) {
                Ok(msg) => msg,
                Err(error) => return self.check_expected_error(&error),
            };

            // Test validation
            let validation_result = signed_msg.validate();

            // Encode message
            let encoded_msg = if validation_result.is_ok() {
                match signed_msg.ssv_message().as_ssz_bytes().len() {
                    0 => return self.check_expected_error("SSVMessage data is empty"),
                    _ => signed_msg.ssv_message().as_ssz_bytes(),
                }
            } else {
                return self.check_validation_error(&validation_result);
            };

            // Check RSA signature if we have public keys
            if let Some(ref pk_strings) = self.rsa_public_key {
                for (i, pk_string) in pk_strings.iter().enumerate() {
                    // Use operator_key to parse the RSA public key from base64
                    let rsa_key = match public::from_base64(pk_string.as_bytes()) {
                        Ok(key) => key,
                        Err(_) => {
                            return self.check_expected_error("failed to parse RSA public key");
                        }
                    };

                    // Convert to PKey for verification
                    let pkey = match PKey::from_rsa(rsa_key) {
                        Ok(key) => key,
                        Err(_) => return self.check_expected_error("failed to convert RSA key"),
                    };

                    // Get signature for this operator
                    if i >= signed_msg.signatures().len() {
                        return self.check_expected_error("not enough signatures for operators");
                    }

                    let signature = &signed_msg.signatures()[i];

                    // Verify signature using PKCS1v15 padding with SHA256
                    let mut verifier = match Verifier::new(MessageDigest::sha256(), &pkey) {
                        Ok(v) => v,
                        Err(_) => return self.check_expected_error("failed to create verifier"),
                    };

                    if let Err(_) = verifier.update(&encoded_msg) {
                        return self.check_expected_error("failed to update verifier");
                    }

                    let signature_bytes: &[u8] = signature;
                    if let Err(_) = verifier.verify(signature_bytes) {
                        return self.check_expected_error("RSA signature verification failed");
                    }
                }
            }

            // If we get here without error but expected one, check if test expects error
            if !self.expected_error.is_empty() {
                return false; // Expected error but didn't get one
            }
        }

        // Check if we expected an error but didn't get one
        self.expected_error.is_empty()
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

        // Convert full data if present
        let full_data = match &test_msg.full_data {
            Some(data_str) => BASE64_STANDARD
                .decode(data_str.as_bytes())
                .map_err(|_| "failed to decode base64 full data")?,
            None => Vec::new(),
        };

        // Create SignedSSVMessage
        SignedSSVMessage::new_from_vecs(
            signatures,
            test_msg.operator_ids.clone(),
            ssv_message.clone(),
            full_data,
        ).map_err(|e| {
            // Map Rust errors to Go error messages
            match e {
                ssv_types::message::SignedSSVMessageError::NoSigners => "no signers".to_string(),
                ssv_types::message::SignedSSVMessageError::ZeroSigner => "signer ID 0 not allowed".to_string(),
                ssv_types::message::SignedSSVMessageError::DuplicatedSigner => "non unique signer".to_string(),
                ssv_types::message::SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => "number of signatures is different than number of signers".to_string(),
                ssv_types::message::SignedSSVMessageError::NoSignatures => "no signatures".to_string(),
                _ => e.to_string(),
            }
        })
    }

    fn check_validation_error(
        &self,
        result: &Result<(), ssv_types::message::SignedSSVMessageError>,
    ) -> bool {
        if self.expected_error.is_empty() {
            return false; // Got error but didn't expect one
        }

        match result {
            Err(err) => {
                let error_str = err.to_string();
                // Map Rust errors to Go error messages
                let go_error = match err {
                    ssv_types::message::SignedSSVMessageError::NoSigners => "no signers",
                    ssv_types::message::SignedSSVMessageError::ZeroSigner => "signer ID 0 not allowed",
                    ssv_types::message::SignedSSVMessageError::DuplicatedSigner => "non unique signer",
                    ssv_types::message::SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => "number of signatures is different than number of signers",
                    ssv_types::message::SignedSSVMessageError::NoSignatures => "no signatures",
                    ssv_types::message::SignedSSVMessageError::SSVMessageError(ssv_types::message::SSVMessageError::EmptyData) => "nil ssvmessage",
                    _ => &error_str,
                };

                self.expected_error == go_error
            }
            Ok(_) => false,
        }
    }

    fn check_expected_error(&self, error_msg: &str) -> bool {
        !self.expected_error.is_empty() && self.expected_error == error_msg
    }
}
