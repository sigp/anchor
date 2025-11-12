use std::fs;

pub use cli::{KeygenSubcommands, Keysplit, Manual, Onchain};
use error::KeysplitError;
use global_config::GlobalConfig;
use openssl::{pkey::Public, rsa::Rsa};
use rayon::prelude::*;
use tracing::info;
use types::{PublicKey, SecretKey};

use crate::{
    crypto::{encrypt_keyshares, split_key},
    output::OutputData,
    split::{manual_split, onchain_split},
    util::read_password,
};

mod cli;
mod crypto;
mod error;
pub mod output;
mod split;
mod util;

// A specific operators keyshare
pub struct KeyShare {
    id: u64,
    public_key: Rsa<Public>,
    keyshare: SecretKey,
}

// A keyshare where the secretkey has been encrypted with the operators public key
pub struct EncryptedKeyShare {
    id: u64,
    public_key: Rsa<Public>,
    share_public_key: PublicKey,
    encrypted_keyshare: Vec<u8>,
}

pub fn run_keysplitter(
    keysplit: Keysplit,
    global_config: GlobalConfig,
) -> Result<(), KeysplitError> {
    let shared = keysplit.get_shared();
    info!("----- Anchor Keysplitter -----");

    // 1) Read in the keystore files and parse them into a usable format
    let keystores = shared
        .keystore_paths
        .iter()
        .map(|path| {
            info!("Reading in validator keystore file from {path}...",);
            eth2_keystore::Keystore::from_json_file(path).map_err(|e| {
                KeysplitError::Keystore(format!("Failed to read keystore file: {e:?}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    info!("Successfully read in validator keystore file(s)");

    // 2) Extract the validator keys from the keystore file
    info!("Extracting keys from keystore file(s)...");
    let password = read_password(shared.password_file.as_deref())
        .map_err(|e| KeysplitError::Keystore(format!("Unable to get password: {e}")))?;
    let keys = keystores
        .into_par_iter()
        .map(|keystore| {
            keystore.decrypt_keypair(password.as_bytes()).map_err(|e| {
                KeysplitError::Keystore(format!("Failed to decrypt keystore file: {e:?}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    info!("Successfully extracted keys from keystore file(s)");

    // 3) Split the key into keyshares and group together relevant information
    info!(
        "Splitting validator key(s) into {} shares...",
        shared.operators.0.len()
    );
    let splits = match keysplit.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual, keys.iter().map(|k| &k.sk)),
        KeygenSubcommands::Onchain(onchain) => {
            onchain_split(onchain, global_config, keys.iter().map(|k| &k.sk))
        }
    }?;
    info!("Successfully split validator key into shares");

    // 4) Encrypt the keyshares with the operators public keys
    info!("Encrypting keyshares...");
    let encrypted_keyshares = splits
        .into_par_iter()
        .map(encrypt_keyshares)
        .collect::<Result<Vec<_>, _>>()?;
    info!("Encrypted all keyshares!");

    // 5) Construct the payload and turn data into proper output format.
    info!(
        "Constructing output and writing to file {}...",
        shared.output_path
    );
    let output = OutputData::new(encrypted_keyshares, &shared, keys)?;

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
