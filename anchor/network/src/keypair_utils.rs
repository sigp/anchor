use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

use libp2p::identity::{Keypair, secp256k1};
use tracing::{debug, warn};

/// Loads a private key from disk. If this fails, a new key is
/// generated and is then saved to disk.
///
/// Currently only secp256k1 keys are allowed, as these are the only keys supported by discv5.
pub fn load_private_key(network_key_file: &Path) -> Keypair {
    // check for key from disk
    if let Ok(mut network_key_file) = File::open(network_key_file) {
        let mut key_bytes: Vec<u8> = Vec::with_capacity(36);
        match network_key_file.read_to_end(&mut key_bytes) {
            Err(_) => debug!("Could not read network key file"),
            Ok(_) => {
                // only accept secp256k1 keys for now
                if let Ok(secret_key) = secp256k1::SecretKey::try_from_bytes(&mut key_bytes) {
                    let kp: secp256k1::Keypair = secret_key.into();
                    debug!("Loaded network key from disk.");
                    return kp.into();
                } else {
                    debug!("Network key file is not a valid secp256k1 key");
                }
            }
        }
    }

    // if a key could not be loaded from disk, generate a new one and save it
    let local_private_key = secp256k1::Keypair::generate();
    match File::create(network_key_file)
        .and_then(|mut f| f.write_all(&local_private_key.secret().to_bytes()))
    {
        Ok(_) => {
            debug!("New network key generated and written to disk");
        }
        Err(e) => {
            warn!(
                file = ?network_key_file,
                error = ?e,
                "Could not write node key to file"
            );
        }
    }
    local_private_key.into()
}
