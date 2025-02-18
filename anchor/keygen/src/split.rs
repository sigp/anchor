use crate::{split_keys, KeyShare, KeygenError, Manual, Onchain};
use types::SecretKey;

pub fn manual_split(
    manual: Manual,
    secret_key: SecretKey,
) -> Result<(Vec<KeyShare>, u64), KeygenError> {
    // We have a key for each operator, join that with its corresponding OperatorId and PublicKey.
    // The keys are ordered as they were input into the cli, we have to assume that this is valid as
    // there is no way to confirm this in manual split mode
    let split_keys = split_keys(&manual.shared, secret_key)?;

    // zip the split keys with the rsa public keys and convert into keyshares. A keyshare is just a
    // split key with the corresponding rsa public key
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
