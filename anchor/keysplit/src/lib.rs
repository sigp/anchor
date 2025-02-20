use crate::crypto::{encrypt_keyshares, split_keys};
use crate::output::OutputData;
use crate::split::{manual_split, onchain_split};
pub use cli::{KeygenSubcommands, Keysplit, Manual, Onchain};
use crypto::extract_key;
use error::KeysplitError;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use std::fs;
use std::fs::File;
use tracing::info;
use types::{PublicKey, SecretKey};

mod cli;
mod crypto;
mod error;
mod keystore;
mod output;
mod split;
mod util;

// A specific operators keyshare
pub(crate) struct KeyShare {
    id: u64,
    public_key: Rsa<Public>,
    keyshare: SecretKey,
}

// A keyshare where the secretkey has been encrypted with the operators public key
pub(crate) struct EncryptedKeyShare {
    id: u64,
    public_key: Rsa<Public>,
    share_public_key: PublicKey,
    encrypted_keyshare: Vec<u8>,
}

// PK and SK from keystore file
struct ValidatorKeys {
    public_key: PublicKey,
    secret_key: SecretKey,
}

pub fn run_keysplitter(keysplit: Keysplit) -> Result<(), KeysplitError> {
    let shared = keysplit.get_shared().clone();
    info!("----- Anchor Keysplitter -----");

    // 1) Read in the keystore file and parse it into a usable format
    info!(
        "Reading in validator keystore file from {}...",
        shared.keystore_path
    );
    let keystore_file = File::open(shared.keystore_path.clone())
        .map_err(|e| KeysplitError::Keystore(format!("Failed to open keystore file: {e}")))?;
    let keystore = keystore::parse_keystore(keystore_file)?;
    info!("Successfully read in validator keystore file");

    // 2) Extract the validator keys from the keystore file
    info!("Extracting keys from keystore file...");
    let keys = extract_key(&keystore, &shared.password)?;
    info!("Successfully extracted keys from keystore file");

    // 3) Split the key into keyshares and group together relevant information
    info!(
        "Splitting validator key into {} shares...",
        shared.operators.0.len()
    );
    let (keyshares, nonce) = match keysplit.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual, keys.secret_key.clone()),
        KeygenSubcommands::Onchain(onchain) => onchain_split(onchain, keys.secret_key.clone()),
    }?;
    info!("Successfully split validator key into shares");

    // 4) Encrypt the keyshares with the operators public keys
    info!("Encrypting keyshares...");
    let encrypted_keyshares = encrypt_keyshares(keyshares)?;
    info!("Encrypted all keyshares!");

    // 5) Construct the payload and turn data into proper output format.
    info!(
        "Constructing output and writing to file {}...",
        shared.output_path
    );
    let output = OutputData::new(encrypted_keyshares, shared.clone(), keys, nonce);

    // 6) Write output data to file
    let json_data = serde_json::to_string_pretty(&output).map_err(|e| {
        KeysplitError::Output(format!("Failed to convert output data to json string: {e}"))
    })?;
    fs::write(shared.output_path, json_data).map_err(|e| {
        KeysplitError::Output(format!("Failed to write output data to output path: {e}"))
    })?;
    info!("Key splitting complete");

    Ok(())
}
