use std::{fs, io, path::PathBuf, string::FromUtf8Error};

use base64::prelude::*;
use clap::Parser;
use openssl::{error::ErrorStack, pkey::Private, rsa::Rsa};
use serde::Serialize;
use thiserror::Error;
use tracing::info;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::encryption::{EncryptionError, encrypt};

pub mod encryption;

#[derive(Error, Debug)]
pub enum KeygenError {
    #[error("Failed to generate new private key: {0}")]
    Generate(#[source] ErrorStack),

    #[error("Failed to convert key to PEM: {0}")]
    Pem(#[source] ErrorStack),

    #[error("Failed to write output: {0}")]
    Output(#[from] io::Error),

    #[error("Failed to convert to UTF8: {0}")]
    Utf8(#[from] FromUtf8Error),

    #[error("Failed to convert output data to JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Encryption error: {0}")]
    Encryption(#[from] EncryptionError),

    #[error("{0}")]
    Custom(String),
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

    #[clap(long, help = "Password for file encryption", value_name = "PASSWORD")]
    pub password: Option<String>,
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

    let public_pem = private_key.public_key_to_pem().map_err(KeygenError::Pem)?;

    let public_pem_string = String::from_utf8(public_pem)?;
    // TODO: Fix RSA headers and implement legacy support
    let public_pem = public_pem_string
        .replace(
            "-----BEGIN PUBLIC KEY-----",
            "-----BEGIN RSA PUBLIC KEY-----",
        )
        .replace("-----END PUBLIC KEY-----", "-----END RSA PUBLIC KEY-----");

    // Encode them to onchain format
    let private_pem_encoded = Zeroizing::new(BASE64_STANDARD.encode(&private_pem));
    let public_pem_encoded = BASE64_STANDARD.encode(&public_pem);

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
        // If a password was provided, encrypt the private key
        if let Some(password) = keygen.password {
            // Encrypt the private key
            let encrypted_private = encrypt(&private_pem, &password)?;

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

#[cfg(test)]
mod keygen_test {
    use super::*;
    use crate::encryption::decrypt_bytes;

    #[test]
    // Make sure decrypted output equals encrypted input and output is valid key
    fn test_encrypt_decrypt() {
        // Generate a random key
        let private_key = Rsa::generate(2048).unwrap();
        let private_pem = private_key.private_key_to_pem().unwrap();
        let private_utf8 = String::from_utf8(private_pem.clone()).unwrap();

        let encrypted = encrypt(&private_pem, "password").unwrap();
        let decrypted = decrypt_bytes("password", &encrypted).unwrap();

        // Make sure it is the same as the original
        assert_eq!(private_utf8, decrypted);

        // Make sure we can construct a key from the output
        assert!(Rsa::private_key_from_pem(decrypted.as_ref()).is_ok());
    }
}
