use base64::prelude::*;
use operator_key::{encrypted::EncryptedKey, unencrypted};
use serde::Deserialize;

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType, utils::deserializers::type_parse::*,
};

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
}

impl EncryptionSpecTest {}

impl SpecTest for EncryptionSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        // Parse the private key using operator_key's unencrypted module
        let sk_pem_base64 = BASE64_STANDARD.encode(&self.sk_pem);
        let private_key = match unencrypted::from_base64(sk_pem_base64.as_bytes()) {
            Ok(key) => key,
            Err(_) => return false,
        };

        // Use the plaintext as a password to test the actual client encryption logic
        // If it's not valid UTF-8, base64 encode it to make it a valid password string
        let password = String::from_utf8(self.plain_text.clone())
            .unwrap_or_else(|_| BASE64_STANDARD.encode(&self.plain_text));

        // Test the actual client key encryption logic: encrypt the private key with the password
        let encrypted_key = match EncryptedKey::encrypt(&private_key, &password) {
            Ok(key) => key,
            Err(_) => return false,
        };

        // Test the actual client key decryption logic: decrypt back to the original key
        let decrypted_key = match encrypted_key.decrypt(&password) {
            Ok(key) => key,
            Err(_) => return false,
        };

        // Verify round-trip: decrypted key should match original private key
        if private_key.p() != decrypted_key.p() || private_key.q() != decrypted_key.q() {
            return false;
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Encryption)
    }
}
