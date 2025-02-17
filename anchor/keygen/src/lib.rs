pub use cli::{Keygen, KeygenSubcommands, Manual, Onchain};

use crate::manual::manual_split;
use crate::onchain::onchain_split;
use crate::util::serialize_rsa;
use bls_lagrange::{split, split_with_rng, KeyId};
use cli::SharedKeygenOptions;
use crypto::extract_keys;
use error::KeygenError;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use rand::rngs::StdRng;
use rand::SeedableRng;
use serde::Serialize;
use std::fs::File;
use types::{Address, PublicKey, SecretKey};

// [signature | public keys | encrypted keys].
mod cli;
mod crypto;
mod error;
mod keystore;
mod manual;
mod onchain;
mod util;

#[derive(Debug, Serialize)]
struct OutputData {
    // tood!() version
    // todo!() created at
    shares: Vec<KeyShare>,
}

#[derive(Debug, Serialize)]
struct KeyShare {
    pub data: KeyData,
    pub payload: Payload,
}

#[derive(Debug, Serialize)]
struct Payload {
    public_key: PublicKey,
    operator_ids: Vec<u64>,
    shares_data: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct KeyData {
    owner_nonce: u64,
    owner_address: Address,
    public_key: PublicKey,
    operators: Vec<Operator>,
}

#[derive(Debug, Serialize)]
struct Operator {
    id: u64,
    #[serde(serialize_with = "serialize_rsa")]
    public_key: Rsa<Public>,
}

pub struct EncryptedKeyShare {
    public_key: Rsa<Public>,
    encrypted_key: Vec<u8>,
}

// Re-direct to manual or onchain keysplitting
pub fn start_keysplitter(keygen: Keygen) -> Result<(), KeygenError> {
    let encrypted_keys = match keygen.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual),
        KeygenSubcommands::Onchain(onchain) => onchain_split(onchain),
    }?;

    //let _json_data = serde_json::to_string_pretty(&output_data);

    // todo!() write to the output file
    Ok(())
}

struct SplitKey {
    id: u64,
    keyshare: SecretKey,
}

// Perform base processing that is relevant to onchain and manual keysplitting
fn base_processing(shared: &SharedKeygenOptions) -> Result<Vec<SplitKey>, KeygenError> {
    // parse the input into a keystore representation for internal use
    let keystore_file = File::open(shared.keystore_path.clone()).unwrap();
    let keystore = keystore::parse_keystore(keystore_file)?;

    // From the keystore file, extract the validators keys
    let validator_keys = extract_keys(&keystore, &shared.password);

    // Once we have the secret key, we can split it into shares
    let key_ids = shared
        .operators
        .0
        .iter()
        .map(|id| KeyId::try_from(*id).unwrap());
    let keys = split(
        validator_keys.secret_key,
        ((shared.operators.0.len() - 1) / 3) as u64,
        key_ids,
    )
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
