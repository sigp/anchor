use bls_lagrange::{KeyId, split};
use openssl::{encrypt::Encrypter, pkey::PKey};
use types::SecretKey;

use crate::{EncryptedKeyShare, KeyShare, KeysplitError, cli::SharedKeygenOptions};

// Given a secret key, split it into parts
pub fn split_keys(
    shared: &SharedKeygenOptions,
    sk: SecretKey,
) -> Result<Vec<(KeyId, SecretKey)>, KeysplitError> {
    let num_operators = shared.operators.0.len();
    let threshold = num_operators - ((num_operators - 1) / 3);

    // Once we have the secret key, we can split it into shares
    let key_ids = shared
        .operators
        .0
        .iter()
        .map(|id| KeyId::try_from(*id).unwrap());

    split(&sk, threshold as u64, key_ids)
        .map_err(|e| KeysplitError::SplitFailure(format!("Failed to split key: {e:?}")))
}

// Encrypt the keyshare with the operators rsa public key
pub fn encrypt_keyshares(
    key_shares: Vec<KeyShare>,
) -> Result<Vec<EncryptedKeyShare>, KeysplitError> {
    key_shares
        .into_iter()
        .map(|share| {
            let pkey = PKey::from_rsa(share.public_key.clone())
                .map_err(|e| KeysplitError::Misc(format!("Failed to map from rsa to pkey: {e}")))?;
            let encrypter = Encrypter::new(&pkey).map_err(|e| {
                KeysplitError::Misc(format!("Failed to construct encrypter with pkey: {e}"))
            })?;

            let data = share.keyshare.serialize();
            let hex_string = hex::encode(&data);
            let data = hex_string.as_bytes();

            let buffer_len = encrypter.encrypt_len(data).map_err(|e| {
                KeysplitError::Misc(format!("Failed to set encryption length: {e}"))
            })?;
            let mut encrypted = vec![0; buffer_len];

            // Encrypt and truncate the buffer
            let encrypted_len = encrypter
                .encrypt(data, &mut encrypted)
                .map_err(|e| KeysplitError::Misc(format!("Failed to perform encryption: {e}")))?;
            encrypted.truncate(encrypted_len);

            Ok(EncryptedKeyShare {
                id: share.id,
                public_key: share.public_key,
                encrypted_keyshare: encrypted,
                share_public_key: share.keyshare.public_key(),
            })
        })
        .collect()
}
