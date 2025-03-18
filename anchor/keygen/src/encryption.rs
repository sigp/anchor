use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use pbkdf2::hmac;
use rand::{rngs::OsRng, RngCore};
use std::{
    fs::File,
    io::{Read, Write},
};

pub fn encrypt(input: &str, password: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Generate a random salt
    let mut salt = [0u8; 16];
    //OsRng.fill_bytes(&mut salt);

    // Derive a key from the password using PBKDF2
    let mut derived_key = [0u8; 32]; // 256 bits
    pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(
        password.as_bytes(),
        &salt,
        10000, // Number of iterations
        &mut derived_key,
    );

    // Generate a random nonce
    let mut nonce_bytes = [0u8; 12]; // 96 bits
                                     //OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Initialize the cipher
    let cipher = Aes256Gcm::new_from_slice(&derived_key)?;

    // Encrypt the data
    let ciphertext = cipher
        .encrypt(nonce, input.as_bytes())
        .map_err(|err| format!("Encryption failed: {}", err))?;

    Ok(ciphertext)
}

pub fn decrypt(password: &str, file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Read the file
    let mut file = File::open(file_path)?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;

    // Extract the salt
    let salt = &contents[0..16];

    // Extract the nonce
    let nonce = Nonce::from_slice(&contents[16..28]);

    // Extract the ciphertext
    let ciphertext = &contents[28..];

    // Derive the key from the password
    let mut derived_key = [0u8; 32]; // 256 bits
    pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(
        password.as_bytes(),
        salt,
        10000, // Number of iterations
        &mut derived_key,
    );

    // Initialize the cipher
    let cipher = Aes256Gcm::new_from_slice(&derived_key)?;

    // Decrypt the data
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|err| format!("Decryption failed: {}", err))?;

    // Convert to a string
    let decrypted = String::from_utf8(plaintext)?;

    Ok(decrypted)
}
