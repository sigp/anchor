use std::{fs, io, path::PathBuf, string::FromUtf8Error};

use base64::prelude::*;
use clap::Parser;
use openssl::{error::ErrorStack, pkey::Private, rsa::Rsa};
use operator_key::ConversionError;
use serde::Serialize;
use thiserror::Error;
use tracing::{error, info};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub mod encryption;
use crate::encryption::{EncryptionError, encrypt};

#[derive(Error, Debug)]
pub enum KeygenError {
    #[error("Failed to generate new private key: {0}")]
    Generate(#[source] ErrorStack),

    #[error("Failed to convert key to PEM: {0}")]
    Pem(#[source] ErrorStack),

    #[error("Failed to read password: {0}")]
    Password(#[from] io::Error),

    #[error("Failed to convert to UTF8: {0}")]
    Utf8(#[from] FromUtf8Error),

    #[error("Failed to convert output data to JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Encryption error: {0}")]
    Encryption(#[from] EncryptionError),

    #[error("{0}")]
    Custom(String),

    #[error("Conversion Error: {0}")]
    Conversion(#[from] ConversionError),
}

#[derive(Zeroize, ZeroizeOnDrop, PartialEq, Debug)]
pub struct SecurePassword(pub String);
impl SecurePassword {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Parser, Clone, Debug)]
#[clap(name = "keygen", about = "RSA key generation tool")]
pub struct Keygen {
    #[clap(long, help = "Path to output keys to", value_name = "OUTPUT_PATH")]
    pub output_path: Option<String>,

    #[clap(
        long,
        help = "Force file overwrite",
        value_name = "FORCE",
        default_value = "false"
    )]
    pub force: bool,

    #[clap(long, help = "Enable password encryption")]
    pub password: bool,
}

#[derive(Debug, Serialize, Zeroize, ZeroizeOnDrop)]
struct PrettyOutput {
    #[zeroize(skip)]
    public: String,
    private: String,
}

// Run RSA keygeneration
pub fn run_keygen(keygen: Keygen) -> Result<Rsa<Private>, KeygenError> {
    // Generate the new rsa private key
    let private_key = Rsa::generate(2048).map_err(KeygenError::Generate)?;

    // Extract the PEM of the public and private keys
    let private_pem = Zeroizing::new(private_key.private_key_to_pem().map_err(KeygenError::Pem)?);

    let private_pem_encoded = Zeroizing::new(BASE64_STANDARD.encode(&private_pem));
    let public_pem_encoded = operator_key::public::to_base64(&private_key)?;

    // Determine the output directory
    let output_dir = if let Some(output_path) = keygen.output_path {
        PathBuf::from(output_path)
    } else {
        PathBuf::from(".") // Current working directory
    };

    // Create output paths for both files
    let pem_file = output_dir.join("key.pem");
    let json_file = output_dir.join("keys.json");

    if keygen.force || (!pem_file.exists() && !json_file.exists()) {
        // If the user would like to password encrypt the key
        if keygen.password {
            let password = read_password_from_user(true)?;

            // Encrypt the private key
            let encrypted_private = encrypt(&private_pem, password)?;

            fs::write(&pem_file, &encrypted_private)?;
            info!("Encrypted private key written to: {}", pem_file.display());

            // Log the public key
            info!("Generated public key: {}", public_pem_encoded);
        } else {
            info!("Password not supplied. Private key will NOT be encrypted");

            // Otherwise, write out plainkey keys to respective files
            let data = PrettyOutput {
                public: public_pem_encoded,
                private: private_pem_encoded.to_string(),
            };
            let pretty_json = Zeroizing::new(serde_json::to_string_pretty(&data)?);

            fs::write(&pem_file, &private_pem)?;
            info!("Private key written to: {}", pem_file.display());

            fs::write(&json_file, pretty_json)?;
            info!("JSON keys written to: {}", json_file.display());
        }
    } else {
        return Err(KeygenError::Custom(format!(
            "PEM file or JSON file already exist in {}",
            output_dir.display()
        )));
    }

    Ok(private_key)
}

pub fn read_password_from_user(confirm: bool) -> Result<SecurePassword, KeygenError> {
    loop {
        // Prompt for password
        let password = SecurePassword(
            rpassword::prompt_password("Enter password for RSA keyfile: ")
                .map_err(KeygenError::Password)?,
        );

        if !confirm {
            return Ok(password);
        }

        // Confirm password
        let confirmation = SecurePassword(
            rpassword::prompt_password("Re-enter password to confirm: ")
                .map_err(KeygenError::Password)?,
        );

        // Verify passwords match
        if password == confirmation {
            return Ok(password);
        }
        error!("Passwords do not match. Please try again.");
    }
}

#[cfg(test)]
mod keygen_test {
    use operator_key::legacy::decrypt;

    use super::*;

    #[test]
    // Make sure decrypted output equals encrypted input and output is valid key
    fn test_encrypt_decrypt() {
        // Generate a random key
        let private_key = Rsa::generate(2048).unwrap();
        let private_pem = private_key.private_key_to_pem().unwrap();

        let password = SecurePassword(String::from("password"));
        let encrypted = encrypt(&private_pem, password).unwrap();
        let password = SecurePassword(String::from("password"));
        let decrypted = decrypt(&password.0, &encrypted).unwrap();

        // Make sure it is the same as the original by comparing the secret prime factors
        assert_eq!(private_key.p(), decrypted.p());
        assert_eq!(private_key.q(), decrypted.q());
    }
}
