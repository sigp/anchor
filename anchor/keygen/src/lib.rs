use base64::prelude::*;
use clap::Parser;
use openssl::rsa::Rsa;
use std::fs;
use tracing::info;
use serde::Serialize;

#[derive(Debug)]
pub enum KeygenError {
    Generate(String),
    Pem(String),
    Output(String),
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
        .map_err(|e| KeygenError::Pem(format!("Failed to convert private key to PEM: {e}")))?;

    // Encode them to onchain format
    let private_pem = BASE64_STANDARD.encode(private_pem);
    let public_pem = BASE64_STANDARD.encode(public_pem);

    // If there is no output path, just log the key values
    if let Some(output_path) = keygen.output_path {
        let data = PrettyOutput { public: public_pem, private: private_pem};
        let pretty_data = serde_json::to_string_pretty(&data).map_err(|e| {
            KeygenError::Output(format!("Failed to convert output data to json string: {e}"))
        })?;
        fs::write(output_path, pretty_data).map_err(|e| {
            KeygenError::Output(format!("Failed to write keys to output file: {e}"))
        })?;
    } else {
        info!("Public: {}", public_pem);
        info!("Private: {}", private_pem);
    }

    Ok(())
}
