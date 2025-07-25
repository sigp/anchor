use std::fs;

pub use cli::{KeygenSubcommands, Keysplit, Manual, Onchain};
use error::KeysplitError;
use global_config::GlobalConfig;
use openssl::{pkey::Public, rsa::Rsa};
use tracing::info;
use types::{PublicKey, SecretKey};

use crate::{
    crypto::{encrypt_keyshares, split_keys},
    output::OutputData,
    split::{manual_split, onchain_split},
};

mod cli;
mod crypto;
mod error;
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

pub fn run_keysplitter(
    keysplit: Keysplit,
    global_config: GlobalConfig,
) -> Result<(), KeysplitError> {
    let shared = keysplit.get_shared().clone();
    info!("----- Anchor Keysplitter -----");

    // 1) Read in the keystore file and parse it into a usable format
    info!(
        "Reading in validator keystore file from {}...",
        shared.keystore_path
    );
    let keystore = eth2_keystore::Keystore::from_json_file(&shared.keystore_path)
        .map_err(|e| KeysplitError::Keystore(format!("Failed to read keystore file: {e:?}")))?;
    info!("Successfully read in validator keystore file");

    // 2) Extract the validator keys from the keystore file
    info!("Extracting keys from keystore file...");
    let keys = keystore
        .decrypt_keypair(shared.password.as_bytes())
        .map_err(|e| KeysplitError::Keystore(format!("Failed to decrypt keystore file: {e:?}")))?;
    info!("Successfully extracted keys from keystore file");

    // 3) Split the key into keyshares and group together relevant information
    info!(
        "Splitting validator key into {} shares...",
        shared.operators.0.len()
    );
    let (keyshares, nonce) = match keysplit.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual, keys.sk.clone()),
        KeygenSubcommands::Onchain(onchain) => {
            onchain_split(onchain, global_config, keys.sk.clone())
        }
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
