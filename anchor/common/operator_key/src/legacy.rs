//! Support for reading encrypted and unencrypted operator keys as stored by Anchor v0.1.0
//!
//! The unencrypted key format is simply the PKCS1 encoded private key.
//!
//! The encrypted key is formatted as follows:
//!
//! 16 bytes of salt, followed by 12 bytes of nonce, followed by the ciphertext.
//!
//! The salt is used to derive the key from the password using pbkdf2 with 10,000 iterations.
//! The nonce is used as initialization vector for the actual decryption using AES256-GCM.
//! The clear text is the PKCS1 encoded private key.
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use openssl::{pkey::Private, rsa::Rsa};
use pbkdf2::hmac;
use thiserror::Error;

use crate::ConversionError;

#[derive(Debug, Error)]
pub enum DecryptionError {
    #[error("Failed to initialize cipher")]
    Cipher,
    #[error("Failed to derive key with PBKDF2")]
    PBKDF2,
    #[error("Input data too small")]
    InvalidDataSize,
    #[error("Failed to decrypt data")]
    Decrypt,
    #[error("Failed to convert data: {0}")]
    Conversion(#[from] ConversionError),
}

// Decrypt the contents of the file with the password
pub fn decrypt(password: &str, contents: &[u8]) -> Result<Rsa<Private>, DecryptionError> {
    if contents.len() < 28 {
        return Err(DecryptionError::InvalidDataSize);
    }

    // Extract the salt, nonce, and ciphertext
    let salt = &contents[0..16];
    let nonce = Nonce::from_slice(&contents[16..28]);
    let ciphertext = &contents[28..];

    // Derive the key from the password
    let mut derived_key = [0u8; 32]; // 256 bits
    pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(
        password.as_bytes(),
        salt,
        10000, // Number of iterations
        &mut derived_key,
    )
    .map_err(|_| DecryptionError::PBKDF2)?;

    // Initialize the cipher
    let cipher = Aes256Gcm::new_from_slice(&derived_key).map_err(|_| DecryptionError::Cipher)?;

    // Decrypt the data
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| DecryptionError::Decrypt)?;

    // Convert to a key
    let key = from_unencrypted_pem(&plaintext)?;

    Ok(key)
}

pub fn from_unencrypted_pem(pem_data: &[u8]) -> Result<Rsa<Private>, ConversionError> {
    // Making sure this is valid UTF-8 is not strictly necessary (as it is implied by
    // private_key_from_pem), but it is good to know for calling code if this is the issue (as that
    // means the key is likely encrypted).
    let pem_decoded = std::str::from_utf8(pem_data)?;
    let rsa_key = Rsa::private_key_from_pem(pem_decoded.as_bytes())?;
    Ok(rsa_key)
}

#[cfg(test)]
mod tests {
    use base64::prelude::*;

    use super::*;

    #[test]
    fn test_decrypt() {
        let password = "qwe";
        // Generated using Anchor v0.1.0 and base64 encoded to avoid binary in repo
        let contents = include_str!("../test_keys/encrypted_legacy_anchor.txt");
        let contents = BASE64_STANDARD.decode(contents).unwrap();
        decrypt(password, &contents).unwrap();
    }
}
