pub use cli::{Keygen, KeygenSubcommands, Manual, Onchain};
use output::encrypted_to_output;

use crate::split::{manual_split, onchain_split};
use bls_lagrange::{split, KeyId};
use cli::SharedKeygenOptions;
use crypto::extract_key;
use error::KeygenError;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use std::fs::File;
use types::SecretKey;

// [signature | public keys | encrypted keys].
mod cli;
mod crypto;
mod error;
mod keystore;
mod output;
mod split;
mod util;

// Re-direct to manual or onchain keysplitting
pub fn start_keysplitter(keygen: Keygen) -> Result<(), KeygenError> {
    let encrypted_keys = match keygen.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual),
        KeygenSubcommands::Onchain(onchain) => onchain_split(onchain),
    }?;

    // have a set of encrypted keys, turn it into output data
    let output = encrypted_to_output(encrypted_keys);
    let _json_data = serde_json::to_string_pretty(&output);
    // todo!() write to the output file

    Ok(())
}

// A piece of a validato key that has been split
struct SplitKey {
    id: u64,
    keyshare: SecretKey,
}

// An operators keyshare after the validator key has been split
pub struct KeyShare {
    id: u64,
    public_key: Rsa<Public>,
    keyshare: SecretKey,
}

// A keyshare where the secretkey has been encrypted with the operators public key
struct EncryptedKeyShare {
    id: u64,
    public_key: Rsa<Public>,
    encrypted_keyshare: Vec<u8>,
}

// Perform shared functionality between onchain and manual keysplitting
// This includes...
// 1) Reading in the keystore file and parsing it into a usable format
// 2) Extracting the validators secret key from the keystore
// 3) Breaking up the secret key into N un-encrypted shares
fn base_processing(shared: &SharedKeygenOptions) -> Result<Vec<SplitKey>, KeygenError> {
    // parse the input into a keystore representation for internal use
    let keystore_file = File::open(shared.keystore_path.clone()).unwrap();
    let keystore = keystore::parse_keystore(keystore_file)?;

    // From the keystore file, extract the validators keys
    let sk = extract_key(&keystore, &shared.password);

    // Once we have the secret key, we can split it into shares
    let key_ids = shared
        .operators
        .0
        .iter()
        .map(|id| KeyId::try_from(*id).unwrap());
    let keys = split(sk, ((shared.operators.0.len() - 1) / 3) as u64, key_ids)
        .map_err(|e| KeygenError::SplitFailure(format!("Failed to split key: {:?}", e)))?;

    let split_keys = keys
        .into_iter()
        .map(|(id, key)| SplitKey {
            id: u64::from(id),
            keyshare: key,
        })
        .collect();

    Ok(split_keys)
}
