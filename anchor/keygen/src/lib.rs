use crate::crypto::{encrypt_keyshares, split_keys};
use crate::output::OutputData;
use crate::split::{manual_split, onchain_split};
pub use cli::{Keygen, KeygenSubcommands, Manual, Onchain};
use crypto::extract_key;
use error::KeygenError;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use std::fs;
use std::fs::File;
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

pub fn run_keysplitter(keygen: Keygen) -> Result<(), KeygenError> {
    let shared = keygen.get_shared().clone();

    // 1) Read in the keystore file and parse it into a usable format
    let keystore_file = File::open(shared.keystore_path.clone()).unwrap();
    let keystore = keystore::parse_keystore(keystore_file)?;

    // 2) Extract the validator keys from the keystore file
    let keys = extract_key(&keystore, &shared.password)?;

    // 3) Split the key into keyshares and group together relevant information
    let (keyshares, nonce) = match keygen.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual, keys.secret_key.clone()),
        KeygenSubcommands::Onchain(onchain) => onchain_split(onchain, keys.secret_key.clone()),
    }?;

    // 4) Encrypt the keyshared with the operators public keys
    let encrypted_keyshares = encrypt_keyshares(keyshares)?;

    // 5) Construct the payload and turn data into proper output format.
    let output = OutputData::new(encrypted_keyshares, shared.clone(), keys, nonce);

    // 6) Write output data to file
    let json_data = serde_json::to_string_pretty(&output).unwrap();
    fs::write(shared.output_path, json_data).unwrap();

    Ok(())
}
