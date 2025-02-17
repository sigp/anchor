use crate::crypto::encrypt_keyshares;
use crate::{base_processing, EncryptedKeyShare, KeyShare, KeygenError, Manual, Onchain};

pub fn manual_split(manual: Manual) -> Result<Vec<EncryptedKeyShare>, KeygenError> {
    // We have a key for each operator, join that with its corresponding OperatorId and PublicKey.
    // The keys are ordered as they were input into the cli, we have to assume that this is valid as
    // there is no way to confirm this in manual split mode
    let split_keys = base_processing(&manual.shared)?;

    // zip the split keys with the rsa public keys and convert into keyshares. A keyshare is just a
    // split key with the corresponding rsa public key
    let key_shares: Vec<KeyShare> = split_keys
        .into_iter()
        .zip(manual.public_keys)
        .map(|(split_key, rsa)| KeyShare {
            id: split_key.id,
            public_key: rsa,
            keyshare: split_key.keyshare,
        })
        .collect();

    // Encrypt all of the keyshares
    encrypt_keyshares(key_shares)
}

// Split the key using onchain data. This takes human error out of the equation and utilizes data scrapped from the chain
// to input the correct operator public keys and owner nonce
pub fn onchain_split(_onchain: Onchain) -> Result<Vec<EncryptedKeyShare>, KeygenError> {
    // High level steps
    // Sync all of the data from the chain
    // - should this just be in memory or should we crate keysplitting db?? probs keysplitter
    // Group all of the info into some common struct and then split the key
    // impl display on the key struct to easily write it into a file
    todo!()
}
