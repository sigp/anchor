use crate::crypto::{encrypt_keyshares, split_keys};
use crate::output::OutputData;
use crate::split::{manual_split, onchain_split};
pub use cli::{Keygen, KeygenSubcommands, Manual, Onchain};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use crypto::extract_key;
use error::KeygenError;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use std::fs;
use std::fs::File;
use types::{PublicKey, SecretKey};
use tracing::info;

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

pub fn run_keysplitter(keygen: Keygen) -> Result<(), KeygenError> {
    let filter = EnvFilter::builder()
            .parse("info,hyper=off,hyper_util=off,alloy_transport_http=off,reqwest=off,alloy_rpc_client=off,alloy_transport_ws=off,alloy_pubsub=off")
            .expect("filter should be valid");
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
    let shared = keygen.get_shared().clone();
    info!("----- Anchor Keysplitter -----");

    // 1) Read in the keystore file and parse it into a usable format
    info!("Reading in validator keystore file...");
    let keystore_file = File::open(shared.keystore_path.clone()).unwrap();
    let keystore = keystore::parse_keystore(keystore_file)?;
    info!("Successfully read in validator keystore file");

    // 2) Extract the validator keys from the keystore file
    info!("Extracting keys from keystore file...");
    let keys = extract_key(&keystore, &shared.password)?;
    info!("Succuessfully extracted keys from keystore file");

    // 3) Split the key into keyshares and group together relevant information
    info!("Splitting validator key into shares...");
    let (keyshares, nonce) = match keygen.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual, keys.secret_key.clone()),
        KeygenSubcommands::Onchain(onchain) => onchain_split(onchain, keys.secret_key.clone()),
    }?;
    info!("Successfully split validator key into shares");

    // 4) Encrypt the keyshares with the operators public keys
    info!("Encrypting keyshares...");
    let encrypted_keyshares = encrypt_keyshares(keyshares)?;
    info!("Encrypted all keyshares!");

    // 5) Construct the payload and turn data into proper output format.
    let output = OutputData::new(encrypted_keyshares, shared.clone(), keys, nonce);

    // 6) Write output data to file
    let json_data = serde_json::to_string_pretty(&output).unwrap();
    fs::write(shared.output_path, json_data).unwrap();

    Ok(())
}
