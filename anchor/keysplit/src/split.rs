use std::{path::Path, sync::Arc};

use database::NetworkDatabase;
use eth::SsvEventSyncer;
use global_config::GlobalConfig;
use openssl::{pkey::Public, rsa::Rsa};
use ssv_types::domain_type::DomainType;
use types::SecretKey;

use crate::{KeyShare, KeysplitError, Manual, Onchain, cli::SharedKeygenOptions, split_key};

/// A single successfully split validator key. Contains a Vec of the key shares ([`KeyShare`] or
/// [`EncryptedKeyShare`]) and the nonce needed to sign the shares.
pub struct Split<T> {
    pub key_shares: Vec<T>,
    pub nonce: u64,
}

// Split the key with manually input nonce value and rsa public keys
pub fn manual_split<'a>(
    manual: Manual,
    secret_keys: impl IntoIterator<Item = &'a SecretKey>,
) -> Result<Vec<Split<KeyShare>>, KeysplitError> {
    // Make sure num operators == num keys
    if manual.shared.operators.0.len() != manual.public_keys.len() {
        return Err(KeysplitError::InvalidKeyLen(
            "Number of keys does not match number of operators".to_string(),
        ));
    }

    create_keyshares_for_keys(
        manual.nonce,
        &manual.shared,
        secret_keys,
        &manual.public_keys,
    )
}

// Split the key using onchain data. This takes human error out of the equation and utilizes data
// scrapped from the chain to input the correct operator public keys and owner nonce
pub fn onchain_split<'a>(
    onchain: Onchain,
    global_config: GlobalConfig,
    secret_keys: impl IntoIterator<Item = &'a SecretKey>,
) -> Result<Vec<Split<KeyShare>>, KeysplitError> {
    // Construct DB and perform sync
    let db = build_db();
    let mut syncer =
        SsvEventSyncer::new_keysplit(db.clone(), onchain.rpc, global_config.ssv_network);

    // Block on the sync, we cannot proceed until this is finished and this prevents refactoring the
    // entire application into async
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| KeysplitError::Misc(format!("Failed to create a new tokio runtime: {e}")))?;
    runtime.block_on(async { syncer.keysplit_sync().await });

    let public_keys = db
        .get_keys_for_operators(&onchain.shared.operators.0)
        .map_err(|_| {
            KeysplitError::InvalidOperator("One or more operators do not exist".to_string())
        })?;

    let nonce = match db.get_nonce_for_owner(onchain.shared.owner) {
        Ok(Some(n)) => n + 1,
        Ok(None) => 0,
        Err(e) => {
            return Err(KeysplitError::Database(format!(
                "Failed to fetch nonce: {e}"
            )));
        }
    };

    create_keyshares_for_keys(nonce, &onchain.shared, secret_keys, &public_keys)
}

fn create_keyshares_for_keys<'a>(
    nonce: u64,
    shared: &SharedKeygenOptions,
    secret_keys: impl IntoIterator<Item = &'a SecretKey>,
    public_keys: &[Rsa<Public>],
) -> Result<Vec<Split<KeyShare>>, KeysplitError> {
    secret_keys
        .into_iter()
        .enumerate()
        .map(|(i, secret_key)| {
            create_keyshares_for_key(nonce + i as u64, shared, secret_key, public_keys)
        })
        .collect()
}

fn create_keyshares_for_key(
    nonce: u64,
    shared: &SharedKeygenOptions,
    secret_key: &SecretKey,
    public_keys: &[Rsa<Public>],
) -> Result<Split<KeyShare>, KeysplitError> {
    // Split the secret key into N shares
    let split_keys = split_key(shared, secret_key)?;

    // With each keyshare, zip it with its corresponding rsa public key
    Ok(Split {
        key_shares: split_keys
            .into_iter()
            .zip(public_keys.iter())
            .map(|(split_key, rsa)| KeyShare {
                id: u64::from(split_key.0),
                public_key: rsa.clone(),
                keyshare: split_key.1,
            })
            .collect(),
        nonce,
    })
}

// Build a network database for the keysplit
fn build_db() -> Arc<NetworkDatabase> {
    // We do not care about the public key here, so just generate a random one to prevent having to
    // use option
    let rsa = Rsa::generate(2048).expect("Keygen will not fail");
    let public_key =
        Rsa::from_public_components(rsa.n().to_owned().unwrap(), rsa.e().to_owned().unwrap())
            .expect("Keygen will not fail");
    let path = Path::new("keysplit.sqlite");
    // TODO: The way the keysplit currently is implemented, we do not have easy access to the domain
    // type. This is easier once https://github.com/sigp/anchor/pull/347 is merged and irrelevant
    // if we implement https://github.com/sigp/anchor/issues/386.
    Arc::new(
        NetworkDatabase::new(path, &public_key, DomainType([0xff; 4]))
            .expect("Database construction will not fail"),
    )
}
