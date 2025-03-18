use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use pbkdf2::hmac;
use rand::{rngs::OsRng, TryRngCore};
use std::fs::File;
use std::io::Read;

#[derive(Debug)]
pub enum EncryptionError {
    FillBytes(String),
    Encrypt(String),
    Cipher(String),
    PBKDF2(String),
    File(String),
    Decrypt(String),
    Conversion(String),
}

// Encrypt the input with a password
pub fn encrypt(input: &str, password: &str) -> Result<Vec<u8>, EncryptionError> {
    // Generate a random salt
    let mut salt = [0u8; 16];
    OsRng.try_fill_bytes(&mut salt).map_err(|e| {
        EncryptionError::FillBytes(format!("Failed to generate randon salt: {e:?}"))
    })?;

    // Derive a key from the password using PBKDF2
    let mut derived_key = [0u8; 32]; // 256 bits
    pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(
        password.as_bytes(),
        &salt,
        10000, // Number of iterations
        &mut derived_key,
    )
    .map_err(|e| EncryptionError::PBKDF2(format!("Failed to perform pbkdf2: {e:?}")))?;

    // Generate a random nonce
    let mut nonce_bytes = [0u8; 12]; // 96 bits
    OsRng.try_fill_bytes(&mut nonce_bytes).map_err(|e| {
        EncryptionError::FillBytes(format!("Failed to generate randon nonce: {e:?}"))
    })?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Initialize the cipher
    let cipher = Aes256Gcm::new_from_slice(&derived_key)
        .map_err(|e| EncryptionError::Cipher(format!("Failed to initialize cipher: {e:?}")))?;

    // Encrypt the data
    let ciphertext = cipher
        .encrypt(nonce, input.as_bytes())
        .map_err(|e| EncryptionError::Encrypt(format!("Failed to encrypt the data: {e:?}")))?;

    Ok(ciphertext)
}

// Decrypt the contents of the file with the password
pub fn decrypt(password: &str, mut file: File) -> Result<String, EncryptionError> {
    // Read the file
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .map_err(|e| EncryptionError::File(format!("Failed to read in keyfile: {e:?}")))?;

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
    .map_err(|e| EncryptionError::PBKDF2(format!("Failed to perform pbkdf2: {e:?}")))?;

    // Initialize the cipher
    let cipher = Aes256Gcm::new_from_slice(&derived_key)
        .map_err(|e| EncryptionError::Cipher(format!("Failed to initialize cipher: {e:?}")))?;

    // Decrypt the data
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| EncryptionError::Decrypt(format!("Failed to decrypt the password: {e:?}")))?;

    // Convert to a string
    let decrypted = String::from_utf8(plaintext).map_err(|e| {
        EncryptionError::Conversion(format!("Failed to convert key to UTF8 string: {e:?}"))
    })?;
    Ok(decrypted)
}
