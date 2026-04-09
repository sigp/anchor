use base64::prelude::*;
use openssl::{encrypt::Encrypter, pkey::PKey, rsa::Padding};
use operator_key::{public, unencrypted};
use serde::Deserialize;

use crate::{SpecTest, utils::deserializers::deserialize_base64};

/// Mirrors Go's `EncryptionSpecTest.Run()`: parse RSA key pair from PEM,
/// verify SK/PK consistency, then RSA-PKCS1v15 encrypt/decrypt roundtrip.
///
/// Uses the same RSA-PKCS1v15 primitives that `validator_store` uses in
/// production for keyshare decryption, but calls `openssl` directly rather
/// than going through the higher-level wrapper.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EncryptionSpecTest {
    #[serde(rename = "SKPem", deserialize_with = "deserialize_base64")]
    sk_pem: Vec<u8>,
    #[serde(rename = "PKPem", deserialize_with = "deserialize_base64")]
    pk_pem: Vec<u8>,
    #[serde(deserialize_with = "deserialize_base64")]
    plain_text: Vec<u8>,
}

impl SpecTest for EncryptionSpecTest {
    fn run(&self) -> Result<(), String> {
        // Parse private key from PEM.
        let sk_b64 = BASE64_STANDARD.encode(&self.sk_pem);
        let private_key = unencrypted::from_base64(sk_b64.as_bytes())
            .map_err(|e| format!("Failed to parse private key: {e}"))?;

        // Verify derived public key matches fixture.
        // Go compares raw PEM bytes; we compare base64-encoded PEM (equivalent).
        let derived_pk_b64 = public::to_base64(&private_key)
            .map_err(|e| format!("Failed to derive public key: {e}"))?;
        let expected_pk_b64 = BASE64_STANDARD.encode(&self.pk_pem);
        if derived_pk_b64 != expected_pk_b64 {
            return Err(
                "Public key derived from private key does not match fixture PKPem".to_string(),
            );
        }

        // Parse public key from fixture PEM.
        let public_key = public::from_base64(expected_pk_b64.as_bytes())
            .map_err(|e| format!("Failed to parse public key: {e}"))?;

        // RSA-PKCS1v15 encrypt plaintext with public key.
        let pkey = PKey::from_rsa(public_key)
            .map_err(|e| format!("Failed to create PKey from RSA public key: {e}"))?;
        let mut encrypter =
            Encrypter::new(&pkey).map_err(|e| format!("Failed to create encrypter: {e}"))?;
        encrypter
            .set_rsa_padding(Padding::PKCS1)
            .map_err(|e| format!("Failed to set padding: {e}"))?;
        let buffer_len = encrypter
            .encrypt_len(&self.plain_text)
            .map_err(|e| format!("Failed to get encrypt length: {e}"))?;
        let mut ciphertext = vec![0u8; buffer_len];
        let encrypted_len = encrypter
            .encrypt(&self.plain_text, &mut ciphertext)
            .map_err(|e| format!("Encryption failed: {e}"))?;
        ciphertext.truncate(encrypted_len);

        // RSA-PKCS1v15 decrypt with private key.
        let mut decrypted = vec![0u8; private_key.size() as usize];
        let decrypted_len = private_key
            .private_decrypt(&ciphertext, &mut decrypted, Padding::PKCS1)
            .map_err(|e| format!("Decryption failed: {e}"))?;
        decrypted.truncate(decrypted_len);

        // Compare decrypted bytes to original plaintext.
        if decrypted != self.plain_text {
            return Err(format!(
                "Roundtrip failed: decrypted {} bytes, expected {} bytes",
                decrypted.len(),
                self.plain_text.len(),
            ));
        }

        Ok(())
    }
}
