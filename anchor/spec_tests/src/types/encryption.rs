use base64::prelude::*;
use operator_key::{encrypted::EncryptedKey, public, unencrypted};
use serde::Deserialize;

use crate::{SpecTest, utils::deserializers::deserialize_base64};

/// Anchor-specific coverage using Go's `EncryptionSpecTest` fixtures.
///
/// Go tests RSA-PKCS1v15 encrypt/decrypt of arbitrary data. Anchor doesn't use
/// RSA-PKCS1v15; operator keys are protected via EIP-2335 keystores instead.
/// This test exercises that production path: parse the fixture's RSA key,
/// verify the SK/PK pair, then roundtrip the key through EIP-2335 encrypt/decrypt.
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
        if self.plain_text.is_empty() {
            return Err("Fixture plain_text is empty — expected non-empty data".to_string());
        }

        // `from_base64` expects base64-encoded PEM, so re-encode the raw PEM bytes.
        let sk_b64 = BASE64_STANDARD.encode(&self.sk_pem);
        let private_key = unencrypted::from_base64(sk_b64.as_bytes())
            .map_err(|e| format!("Failed to parse private key: {e}"))?;

        // Verify derived public key matches fixture.
        let derived_pk_b64 = public::to_base64(&private_key)
            .map_err(|e| format!("Failed to derive public key: {e}"))?;
        let expected_pk_b64 = BASE64_STANDARD.encode(&self.pk_pem);
        if derived_pk_b64 != expected_pk_b64 {
            return Err(
                "Public key derived from private key does not match fixture PKPem".to_string(),
            );
        }

        // Use plaintext as password; fall back to base64 if not valid UTF-8.
        // `into_bytes()` recovers the original Vec<u8> from the FromUtf8Error.
        let password = String::from_utf8(self.plain_text.clone())
            .unwrap_or_else(|e| BASE64_STANDARD.encode(e.into_bytes()));

        let encrypted = EncryptedKey::encrypt(&private_key, &password)
            .map_err(|e| format!("Encryption failed: {e}"))?;

        let decrypted = encrypted
            .decrypt(&password)
            .map_err(|e| format!("Decryption failed: {e}"))?;

        // Compare full DER-encoded keys (all components: n, e, d, p, q, dp, dq, qinv).
        // Note: Go compares RSA-encrypted plaintext; we compare the key itself after EIP-2335
        // roundtrip.
        let original_der = private_key
            .private_key_to_der()
            .map_err(|e| format!("Failed to encode original key to DER: {e}"))?;
        let decrypted_der = decrypted
            .private_key_to_der()
            .map_err(|e| format!("Failed to encode decrypted key to DER: {e}"))?;
        if original_der != decrypted_der {
            return Err("Roundtrip failed: decrypted key does not match original".to_string());
        }

        Ok(())
    }
}
