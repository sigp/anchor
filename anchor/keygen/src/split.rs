use crate::keysplit_db::KeysplitDatabase;
use crate::keysplit_syncer::KeysplitSyncer;
use crate::{split_keys, KeyShare, KeygenError, Manual, Onchain};
use futures::executor::block_on;
use std::sync::Arc;
use types::SecretKey;

// Split the key with manually input nonce value and rsa public keys
pub fn manual_split(
    manual: Manual,
    secret_key: SecretKey,
) -> Result<(Vec<KeyShare>, u64), KeygenError> {
    // Make sure num operators == num keys
    if manual.shared.operators.0.len() != manual.public_keys.len() {
        return Err(KeygenError::InvalidKeyLen(
            "Number of keys does not match number of operators".to_string(),
        ));
    }

    // Split the secret key into N keyshares
    let split_keys = split_keys(&manual.shared, secret_key)?;

    // With each keyshare, zip it with its corresponding rsa public key
    Ok((
        split_keys
            .into_iter()
            .zip(manual.public_keys)
            .map(|(split_key, rsa)| KeyShare {
                id: u64::from(split_key.0),
                public_key: rsa,
                keyshare: split_key.1,
            })
            .collect(),
        manual.nonce,
    ))
}

// Split the key using onchain data. This takes human error out of the equation and utilizes data scrapped from the chain
// to input the correct operator public keys and owner nonce
pub fn onchain_split(
    onchain: Onchain,
    secret_key: SecretKey,
) -> Result<(Vec<KeyShare>, u64), KeygenError> {
    // Split the secret key into N shares
    let split_keys = split_keys(&onchain.shared, secret_key)?;

    // Construct DB and perform key sync
    let db = Arc::new(KeysplitDatabase::new());
    let syncer = KeysplitSyncer::new(onchain.rpc, db.clone());

    // Block on the sync, we cannot proceed until this is finished and this prevents refactoring the
    // entire application into async
    block_on(async { syncer.sync_data().await });

    let public_keys = db
        .get_keys_for_operators(onchain.shared.operators.0)
        .unwrap();
    let nonce = db.get_nonce_for_owner(onchain.shared.owner);

    // With each keyshare, zip it with its corresponding rsa public key
    Ok((
        split_keys
            .into_iter()
            .zip(public_keys)
            .map(|(split_key, rsa)| KeyShare {
                id: u64::from(split_key.0),
                public_key: rsa,
                keyshare: split_key.1,
            })
            .collect(),
        nonce,
    ))
}
