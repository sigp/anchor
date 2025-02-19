use crate::{split_keys, KeyShare, KeygenError, Manual, Onchain};
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
    _onchain: Onchain,
    _secret_key: SecretKey,
) -> Result<(Vec<KeyShare>, u64), KeygenError> {
    todo!()
}
