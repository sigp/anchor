use openssl::{hash::MessageDigest, pkey::PKey, sign::Verifier};
use operator_key::public;
use serde::Deserialize;
use ssv_types::{
    OperatorId,
    message::{SSVMessage, SignedSSVMessage, SignedSSVMessageError},
};
use ssz::Encode;

use crate::{
    SpecTest, SpecTestType,
    types::TypesSpecTestType,
    utils::deserializers::{deserialize_base64_list, deserialize_hex_option},
};

// Test message structure that directly handles null SSVMessage
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TestSignedSSVMessage {
    #[serde(deserialize_with = "deserialize_base64_list")]
    pub signatures: Vec<Vec<u8>>,
    #[serde(rename = "OperatorIDs")]
    pub operator_ids: Vec<OperatorId>,
    #[serde(rename = "SSVMessage")]
    pub ssv_message: Option<SSVMessage>,
    #[serde(deserialize_with = "deserialize_hex_option", default)]
    pub full_data: Option<Vec<u8>>,
}

// SignedSSVMessage validation tests
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SignedSSVMessageTest {
    pub messages: Vec<TestSignedSSVMessage>,
    pub expected_error: String,
    #[serde(rename = "RSAPublicKey")]
    pub rsa_public_key: Option<Vec<String>>,
}

impl SpecTest for SignedSSVMessageTest {
    fn run(&self) -> bool {
        for test_msg in &self.messages {
            if let Err(error) = self.validate_message(test_msg) {
                return self.check_expected_error(&error);
            }
        }
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SignedSSVMsg)
    }
}

impl SignedSSVMessageTest {
    fn validate_message(&self, test_msg: &TestSignedSSVMessage) -> Result<(), String> {
        // Handle null SSVMessage case
        let ssv_message = test_msg.ssv_message.as_ref().ok_or("nil SSVMessage")?;

        // Convert and validate signatures
        let signatures = self.prepare_signatures(test_msg)?;

        // Create SignedSSVMessage
        let full_data = test_msg.full_data.clone().unwrap_or_default();
        let signed_msg = SignedSSVMessage::new(
            signatures,
            test_msg.operator_ids.clone(),
            ssv_message.clone(),
            full_data,
        )
        .map_err(|e| self.error_to_string(&e))?;

        // Validate the message by calling our internal validate function
        signed_msg
            .validate()
            .map_err(|_| "validation failed".to_string())?;

        // Verify RSA signatures if provided
        self.verify_rsa_signatures(&signed_msg, ssv_message)
    }

    fn prepare_signatures(
        &self,
        test_msg: &TestSignedSSVMessage,
    ) -> Result<Vec<[u8; 256]>, String> {
        let mut signatures = Vec::new();

        for sig_bytes in &test_msg.signatures {
            if sig_bytes.is_empty() {
                return Err("empty signature".to_string());
            }

            // Pad or truncate signature to 256 bytes for RSA signature format
            let mut sig_array = [0u8; 256];
            if sig_bytes.len() <= 256 {
                sig_array[..sig_bytes.len()].copy_from_slice(sig_bytes);
            } else {
                sig_array.copy_from_slice(&sig_bytes[..256]);
            }
            signatures.push(sig_array);
        }

        Ok(signatures)
    }

    fn verify_rsa_signatures(
        &self,
        signed_msg: &SignedSSVMessage,
        ssv_message: &SSVMessage,
    ) -> Result<(), String> {
        let Some(ref pk_strings) = self.rsa_public_key else {
            return Ok(());
        };

        let encoded_ssv_msg = ssv_message.as_ssz_bytes();

        for (i, pk_string) in pk_strings.iter().enumerate() {
            let rsa_key = public::from_base64(pk_string.as_bytes())
                .map_err(|_| "failed to parse RSA public key")?;

            let pkey = PKey::from_rsa(rsa_key).map_err(|_| "failed to convert RSA key to PKey")?;

            let mut verifier = Verifier::new(MessageDigest::sha256(), &pkey)
                .map_err(|_| "failed to create verifier")?;

            verifier
                .update(&encoded_ssv_msg)
                .map_err(|_| "failed to update verifier")?;

            let signature: &[u8] = &signed_msg.signatures()[i];
            verifier
                .verify(signature)
                .map_err(|_| "signature verification failed")?;
        }

        Ok(())
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
            SignedSSVMessageError::TooManySignatures { .. } => "too many signatures".to_string(),
            SignedSSVMessageError::WrongRSASignatureSize { .. } => {
                "wrong RSA signature size".to_string()
            }
            SignedSSVMessageError::TooManyOperatorIDs { .. } => "too many operator IDs".to_string(),
            SignedSSVMessageError::FullDataTooLong { .. } => "full data too long".to_string(),
            SignedSSVMessageError::SignersNotSorted => "signers not sorted".to_string(),
            SignedSSVMessageError::SSVMessageError(_) => "invalid SSV message".to_string(),
        }
    }

    fn check_expected_error(&self, error_msg: &str) -> bool {
        !self.expected_error.is_empty() && self.expected_error == error_msg
    }
}
