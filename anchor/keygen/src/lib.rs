use base64::prelude::*;
use clap::Parser;
use openssl::rsa::Rsa;
use serde::Serialize;
use std::{fs, path::PathBuf};
use tracing::info;

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
}

#[derive(Debug, Serialize)]
struct PrettyOutput {
    public: String,
    private: String,
}

// Run RSA keygeneration
pub fn run_keygen(keygen: Keygen) -> Result<(), KeygenError> {
    // Generate the new rsa private key
    let private_key = Rsa::generate(2048)
        .map_err(|e| KeygenError::Generate(format!("Failed to generate new private key: {e}")))?;

    // Extract the PEM of the public and private keys
    let private_pem = private_key
        .private_key_to_pem()
        .map_err(|e| KeygenError::Pem(format!("Failed to convert private key to PEM: {e}")))?;

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
    let private_pem_encoded = BASE64_STANDARD.encode(&private_pem);
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

    // Write the PEM file
    fs::write(&pem_file, &private_pem).map_err(|e| {
        KeygenError::Output(format!("Failed to write private key to PEM file: {e}"))
    })?;

    info!("Private key written to: {}", pem_file.display());

    // Create JSON data structure
    let data = PrettyOutput {
        public: public_pem_encoded,
        private: private_pem_encoded,
    };

    // Convert to pretty JSON
    let pretty_json = serde_json::to_string_pretty(&data).map_err(|e| {
        KeygenError::Output(format!("Failed to convert output data to JSON string: {e}"))
    })?;

    // Write the JSON file
    fs::write(&json_file, pretty_json)
        .map_err(|e| KeygenError::Output(format!("Failed to write keys to JSON file: {e}")))?;

    info!("JSON keys written to: {}", json_file.display());

    Ok(())
}
