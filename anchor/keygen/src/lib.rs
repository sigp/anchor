use base64::prelude::*;
use clap::Parser;
use openssl::{pkey::Private, rsa::Rsa};
use serde::Serialize;
use std::{fs, path::PathBuf};
use tracing::info;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Debug)]
pub enum KeygenError {
    Generate(String),
    Pem(String),
    Output(String),
    Utf8(String),
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
    let private_key = Rsa::generate(2048)
        .map_err(|e| KeygenError::Generate(format!("Failed to generate new private key: {e}")))?;

    // Extract the PEM of the public and private keys
    let private_pem = Zeroizing::new(
        private_key
            .private_key_to_pem()
            .map_err(|e| KeygenError::Pem(format!("Failed to convert private key to PEM: {e}")))?,
    );

    let public_pem = private_key
        .public_key_to_pem()
        .map_err(|e| KeygenError::Pem(format!("Failed to convert public key to PEM: {e}")))?;

    let public_pem_string = String::from_utf8(public_pem)
        .map_err(|e| KeygenError::Utf8(format!("Failed to convert public key to UTF8: {e}")))?;
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

    // Create JSON data structure
    let data = PrettyOutput {
        public: public_pem_encoded,
        private: private_pem_encoded.to_string(),
    };

    // Convert to pretty JSON
    let pretty_json = Zeroizing::new(serde_json::to_string_pretty(&data).map_err(|e| {
        KeygenError::Output(format!("Failed to convert output data to JSON string: {e}"))
    })?);

    if keygen.force || (!pem_file.exists() && !json_file.exists()) {
        // Write the PEM file
        fs::write(&pem_file, &private_pem).map_err(|e| {
            KeygenError::Output(format!("Failed to write private key to PEM file: {e}"))
        })?;

        info!("Private key written to: {}", pem_file.display());

        // Write the JSON file
        fs::write(&json_file, pretty_json)
            .map_err(|e| KeygenError::Output(format!("Failed to write keys to JSON file: {e}")))?;

        info!("JSON keys written to: {}", json_file.display());
    } else {
        return Err(KeygenError::Output(format!(
            "PEM file or JSON file already exist in {}",
            output_dir.display()
        )));
    }

    Ok(private_key)
}
