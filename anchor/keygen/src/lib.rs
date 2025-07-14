use std::{fs, io, path::PathBuf};

use clap::Parser;
use openssl::{error::ErrorStack, pkey::Private, rsa::Rsa};
use operator_key::{
    ConversionError,
    encrypted::{EncryptedKey, EncryptionError},
    public, unencrypted,
};
use thiserror::Error;
use tracing::{error, info};
use zeroize::Zeroizing;

#[derive(Error, Debug)]
pub enum KeygenError {
    #[error("Failed to generate new private key: {0}")]
    Generate(#[from] ErrorStack),

    #[error("Failed to convert key to PEM: {0}")]
    Conversion(#[from] ConversionError),

    #[error("Failed to read password: {0}")]
    Password(#[source] io::Error),

    #[error("Failed to write key: {0}")]
    KeyOutput(#[source] io::Error),

    #[error("Failed to encrypt the key: {0}")]
    EncryptionError(#[from] EncryptionError),

    #[error("Failed to convert output data to JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Key file(s) already exist in {0}")]
    Exists(String),
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

    #[clap(
        long,
        help = "Enable password encryption. Password is read from terminal or via --password-file"
    )]
    pub encrypt: bool,

    #[clap(
        long,
        help = "Path to a file containing the password to use",
        requires = "encrypt"
    )]
    pub password_file: Option<PathBuf>,
}

// Run RSA keygeneration
pub fn run_keygen(keygen: Keygen) -> Result<Rsa<Private>, KeygenError> {
    // Generate the new rsa private key
    let private_key = Rsa::generate(2048)?;

    let public_key = public::to_base64(&private_key)?;

    // Determine the output directory
    let output_dir = if let Some(output_path) = keygen.output_path {
        PathBuf::from(output_path)
    } else {
        PathBuf::from(".") // Current working directory
    };

    // Create output paths for both files
    let private_key_file = if keygen.encrypt {
        output_dir.join("encrypted_private_key.json")
    } else {
        output_dir.join("private_key.txt")
    };
    let pubkey_file = output_dir.join("public_key.txt");

    if keygen.force || (!private_key_file.exists() && !pubkey_file.exists()) {
        // If the user would like to password encrypt the key
        if keygen.encrypt {
            let password = if let Some(password_file) = keygen.password_file {
                // Zeroize the original allocation
                let full = Zeroizing::new(
                    fs::read_to_string(password_file).map_err(KeygenError::Password)?,
                );
                // Zeroize the allocation with the trimmed string
                Zeroizing::new(full.trim().to_string())
            } else {
                read_password_from_user(true)?
            };

            // Encrypt the private key
            let encrypted_private = EncryptedKey::encrypt(&private_key, &password)?;

            fs::write(&private_key_file, &String::try_from(encrypted_private)?)
                .map_err(KeygenError::KeyOutput)?;
            info!(
                "Encrypted private key written to: {}",
                private_key_file.display()
            );
        } else {
            info!("Password not supplied. Private key will NOT be encrypted");

            fs::write(&private_key_file, &unencrypted::to_base64(&private_key)?)
                .map_err(KeygenError::KeyOutput)?;
            info!("Private key written to: {}", private_key_file.display());
        }

        // Log the public key
        info!("Generated public key: {public_key}");
        fs::write(&pubkey_file, &public_key).map_err(KeygenError::KeyOutput)?;
    } else {
        return Err(KeygenError::Exists(
            output_dir.to_string_lossy().into_owned(),
        ));
    }

    Ok(private_key)
}

pub fn read_password_from_user(confirm: bool) -> Result<Zeroizing<String>, KeygenError> {
    loop {
        // Prompt for password
        let password = Zeroizing::new(
            rpassword::prompt_password("Enter password for keyfile: ")
                .map_err(KeygenError::Password)?,
        );

        if !confirm {
            return Ok(password);
        }

        // Confirm password
        let confirmation = Zeroizing::new(
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
