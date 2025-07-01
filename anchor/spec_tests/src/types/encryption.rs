use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use openssl::{
    pkey::Private,
    rsa::{Padding, Rsa},
};
use serde::Deserialize;

// Encryption test - using existing RSA infrastructure from test utilities
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncryptionSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "SKPem", deserialize_with = "deserialize_base64_to_bytes")]
    pub sk_pem: Vec<u8>,
    #[serde(rename = "PKPem", deserialize_with = "deserialize_base64_to_bytes")]
    pub pk_pem: Vec<u8>,
    #[serde(rename = "PlainText", deserialize_with = "deserialize_base64_to_bytes")]
    pub plain_text: Vec<u8>,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl EncryptionSpecTest {
    // Follow established error matching pattern
    fn is_matching_error(&self, actual_error: &str, expected_error: &str) -> bool {
        if expected_error.is_empty() {
            false
        } else {
            actual_error.contains(expected_error)
        }
    }

    // Perform RSA encryption/decryption round-trip test using existing RSA infrastructure
    fn test_encryption_round_trip(&self) -> Result<(), String> {
        // Parse private key from DER format (similar to rsa_secret_from_hex in utils.rs)
        let private_key = Rsa::private_key_from_der(&self.sk_pem)
            .map_err(|e| format!("Failed to parse private key: {}", e))?;

        // Parse public key from DER format
        let public_key = Rsa::public_key_from_der(&self.pk_pem)
            .map_err(|e| format!("Failed to parse public key: {}", e))?;

        // Test encryption with public key
        let mut encrypted = vec![0; public_key.size() as usize];
        let encrypted_len = public_key
            .public_encrypt(&self.plain_text, &mut encrypted, Padding::PKCS1)
            .map_err(|e| format!("Encryption failed: {}", e))?;
        encrypted.truncate(encrypted_len);

        // Test decryption with private key
        let mut decrypted = vec![0; private_key.size() as usize];
        let decrypted_len = private_key
            .private_decrypt(&encrypted, &mut decrypted, Padding::PKCS1)
            .map_err(|e| format!("Decryption failed: {}", e))?;
        decrypted.truncate(decrypted_len);

        // Verify round-trip: decrypted should match original plain text
        if decrypted != self.plain_text {
            return Err("Round-trip failed: decrypted data does not match original".to_string());
        }

        Ok(())
    }
}

impl SpecTest for EncryptionSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        let has_expected_error = !self.expected_error.is_empty();

        println!("✅ Running encryption test: {}", self.name);

        // Use existing RSA infrastructure for encryption/decryption testing
        match self.test_encryption_round_trip() {
            Ok(()) => {
                if has_expected_error {
                    println!(
                        "❌ Expected error '{}' but encryption succeeded",
                        self.expected_error
                    );
                    false
                } else {
                    println!("✅ Encryption test passed: {}", self.name);
                    println!("   Successfully completed RSA encryption/decryption round-trip");
                    true
                }
            }
            Err(e) => {
                if has_expected_error && self.is_matching_error(&e, &self.expected_error) {
                    println!("✅ Expected encryption failure: {}", e);
                    true
                } else {
                    println!("❌ Unexpected encryption error: {}", e);
                    false
                }
            }
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Encryption)
    }
}
