use std::{io, string::FromUtf8Error};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use pbkdf2::hmac;
use rand::{TryRngCore, rngs::OsRng};
use thiserror::Error;

use super::SecurePassword;

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("Failed to generate random bytes")]
    Random,

    #[error("Failed to encrypt data")]
    Encrypt,

    #[error("Failed to initialize cipher")]
    Cipher,

    #[error("Failed to derive key with PBKDF2")]
    PBKDF2,

    #[error("Failed to read file: {0}")]
    IO(#[from] io::Error),

    #[error("Input data too small")]
    InvalidDataSize,

    #[error("Failed to decrypt data")]
    Decrypt,

    #[error("Failed to convert data: {0}")]
    Conversion(#[from] FromUtf8Error),
}

// Encrypt the input with a password
pub(crate) fn encrypt(input: &[u8], password: SecurePassword) -> Result<Vec<u8>, EncryptionError> {
    // Generate a random salt
    let mut salt = [0u8; 16];
    OsRng
        .try_fill_bytes(&mut salt)
        .map_err(|_| EncryptionError::Random)?;

    // Derive a key from the password using PBKDF2
    let mut derived_key = [0u8; 32];
    pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(
        password.0.as_bytes(),
        &salt,
        10000, // Number of iterations
        &mut derived_key,
    )
    .map_err(|_| EncryptionError::PBKDF2)?;

    // Generate a random nonce
    let mut nonce_bytes = [0u8; 12];
    OsRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|_| EncryptionError::Random)?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Initialize the cipher
    let cipher = Aes256Gcm::new_from_slice(&derived_key).map_err(|_| EncryptionError::Cipher)?;

    // Encrypt the data
    let ciphertext = cipher
        .encrypt(nonce, input)
        .map_err(|_| EncryptionError::Encrypt)?;

    // Combine salt, nonce, and ciphertext into a single output
    let mut output = Vec::with_capacity(salt.len() + nonce_bytes.len() + ciphertext.len());
    output.extend_from_slice(&salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    Ok(output)
}

// Decrypt the contents of the file with the password
pub fn decrypt(password: SecurePassword, contents: &[u8]) -> Result<String, EncryptionError> {
    if contents.len() < 28 {
        return Err(EncryptionError::InvalidDataSize);
    }

    // Extract the salt, nonce, and ciphertext
    let salt = &contents[0..16];
    let nonce = Nonce::from_slice(&contents[16..28]);
    let ciphertext = &contents[28..];

    // Derive the key from the password
    let mut derived_key = [0u8; 32]; // 256 bits
    pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(
        password.0.as_bytes(),
        salt,
        10000, // Number of iterations
        &mut derived_key,
    )
    .map_err(|_| EncryptionError::PBKDF2)?;

    // Initialize the cipher
    let cipher = Aes256Gcm::new_from_slice(&derived_key).map_err(|_| EncryptionError::Cipher)?;

    // Decrypt the data
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| EncryptionError::Decrypt)?;

    // Convert to a string
    let decrypted = String::from_utf8(plaintext)?;
    Ok(decrypted)
}
