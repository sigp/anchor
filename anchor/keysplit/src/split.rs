use std::{path::Path, sync::Arc};

use database::NetworkDatabase;
use eth::SsvEventSyncer;
use openssl::rsa::Rsa;
use types::SecretKey;

use crate::{KeyShare, KeysplitError, Manual, Onchain, cli::Network, split_keys};

// Split the key with manually input nonce value and rsa public keys
pub fn manual_split(
    manual: Manual,
    secret_key: SecretKey,
) -> Result<(Vec<KeyShare>, u64), KeysplitError> {
    // Make sure num operators == num keys
    if manual.shared.operators.0.len() != manual.public_keys.len() {
        return Err(KeysplitError::InvalidKeyLen(
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

// Split the key using onchain data. This takes human error out of the equation and utilizes data
// scrapped from the chain to input the correct operator public keys and owner nonce
pub fn onchain_split(
    onchain: Onchain,
    secret_key: SecretKey,
) -> Result<(Vec<KeyShare>, u64), KeysplitError> {
    // Split the secret key into N shares
    let split_keys = split_keys(&onchain.shared, secret_key)?;

    let network = match onchain.network {
        Network::Holesky => String::from("holesky"),
        Network::Hoodi => String::from("hoodi"),
    };

    // Construct DB and perform sync
    let db = build_db();
    let mut syncer = SsvEventSyncer::new_keysplit(db.clone(), onchain.rpc, network);

    // Block on the sync, we cannot proceed until this is finished and this prevents refactoring the
    // entire application into async
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| KeysplitError::Misc(format!("Failed to create a new tokio runtime: {e}")))?;
    runtime.block_on(async { syncer.keysplit_sync().await });

    let public_keys = db
        .get_keys_for_operators(onchain.shared.operators.0)
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

// Build a network database for the keysplit
fn build_db() -> Arc<NetworkDatabase> {
    // We do not care about the public key here, so just generate a random one to prevent having to
    // use option
    let rsa = Rsa::generate(2048).expect("Keygen will not fail");
    let public_key =
        Rsa::from_public_components(rsa.n().to_owned().unwrap(), rsa.e().to_owned().unwrap())
            .expect("Keygen will not fail");
    let path = Path::new("keysplit.sqlite");
    Arc::new(NetworkDatabase::new(path, &public_key).expect("Database construction will not fail"))
}
