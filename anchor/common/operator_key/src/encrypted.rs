//! The encrypted operator key format
//!
//! A JSON "`crypto`" object as defined in
//! [EIP-2335](https://eips.ethereum.org/EIPS/eip-2335#json-schema), with an additional optional
//! "`pubKey`" property containing the public key as encoded by [`public::to_base64`].
//!
//! Example structure:
//!
//! ```json
//! {
//!   "checksum": {
//!     "function": "sha256",
//!     "message": "...",
//!     "params": {}
//!   },
//!   "cipher": {
//!     "function": "aes-128-ctr",
//!     "message": "...",
//!     "params": {
//!       "iv": "..."
//!     }
//!   },
//!   "kdf": {
//!     "function": "pbkdf2",
//!     "message": "",
//!     "params": {
//!       "c": 262144,
//!       "dklen": 32,
//!       "prf": "hmac-sha256",
//!       "salt": "..."
//!     }
//!   },
//!   "pubKey": "..."
//! }
//! ```
use eth2_keystore::{
    IV_SIZE, SALT_SIZE, default_kdf,
    json_keystore::{
        Aes128Ctr, ChecksumModule, Cipher, CipherModule, Crypto, EmptyMap, EmptyString, KdfModule,
        Sha256Checksum,
    },
};
use openssl::{pkey::Private, rsa::Rsa};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{ConversionError, public};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncryptedKey {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "pubKey")]
    pubkey: Option<String>,
    kdf: KdfModule,
    checksum: ChecksumModule,
    cipher: CipherModule,
}

#[derive(Error, Debug)]
pub enum DecryptionError {
    #[error("Error while decrypting: {0:?}")]
    Keystore(eth2_keystore::Error),
    #[error("OpenSSL error: {0}")]
    OpenSSL(#[from] openssl::error::ErrorStack),
    #[error("Error while reading pubkey from keystore: {0}")]
    InvalidPubkey(#[from] ConversionError),
    #[error("Pubkey stored in keystore does not match the encrypted key")]
    PubkeyDoesNotMatch,
}

#[derive(Error, Debug)]
pub enum EncryptionError {
    #[error("Error while encrypting: {0:?}")]
    Keystore(eth2_keystore::Error),
    #[error("OpenSSL error: {0}")]
    OpenSSL(#[from] openssl::error::ErrorStack),
    #[error("Internal error while converting to pubkey: {0}")]
    PubkeyConversion(#[from] ConversionError),
}

impl EncryptedKey {
    fn as_crypto(&self) -> Crypto {
        Crypto {
            kdf: self.kdf.clone(),
            checksum: self.checksum.clone(),
            cipher: self.cipher.clone(),
        }
    }

    /// Decrypt the private key from the keystore.
    ///
    /// If the pubkey was provided along the encrypted key in a "pubKey" attribute, it is verified
    /// whether the encrypted key matches the public key.
    pub fn decrypt(&self, password: &str) -> Result<Rsa<Private>, DecryptionError> {
        let pem = eth2_keystore::decrypt(password.as_ref(), &self.as_crypto())
            .map_err(DecryptionError::Keystore)?;
        let key = Rsa::private_key_from_pem(pem.as_ref())?;
        if let Some(pubkey) = &self.pubkey {
            let pubkey = public::from_base64(pubkey.as_ref())?;
            if pubkey.e() != key.e() || pubkey.n() != key.n() {
                return Err(DecryptionError::PubkeyDoesNotMatch);
            }
        }
        Ok(key)
    }

    /// Encrypt a private key into a keystore.
    ///
    /// [`Cipher::Aes128Ctr`] is used as cipher, and `scrypt` as constructed by [`default_kdf`] is
    /// used as key derivation function.
    pub fn encrypt(key: &Rsa<Private>, password: &str) -> Result<EncryptedKey, EncryptionError> {
        let pem = Zeroizing::new(key.private_key_to_pem()?);

        let salt = rand::random::<[u8; SALT_SIZE]>();
        let iv = rand::random::<[u8; IV_SIZE]>().to_vec().into();
        let kdf = default_kdf(salt.to_vec());
        let cipher = Cipher::Aes128Ctr(Aes128Ctr { iv });

        let (cipher_text, checksum) =
            eth2_keystore::encrypt(pem.as_ref(), password.as_ref(), &kdf, &cipher)
                .map_err(EncryptionError::Keystore)?;

        Ok(EncryptedKey {
            kdf: KdfModule {
                function: kdf.function(),
                params: kdf,
                message: EmptyString,
            },
            checksum: ChecksumModule {
                function: Sha256Checksum::function(),
                params: EmptyMap,
                message: checksum.to_vec().into(),
            },
            cipher: CipherModule {
                function: cipher.function(),
                params: cipher,
                message: cipher_text.into(),
            },
            pubkey: Some(public::to_base64(key)?),
        })
    }
}

impl TryFrom<EncryptedKey> for String {
    type Error = serde_json::Error;

    fn try_from(value: EncryptedKey) -> Result<Self, Self::Error> {
        serde_json::to_string(&value)
    }
}

impl TryFrom<&str> for EncryptedKey {
    type Error = serde_json::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        serde_json::from_str(value)
    }
}

impl TryFrom<&[u8]> for EncryptedKey {
    type Error = serde_json::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        serde_json::from_slice(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let key = Rsa::generate(2048).unwrap();
        let password = "<PASSWORD>";
        let encrypted = EncryptedKey::encrypt(&key, password).unwrap();
        let decrypted = encrypted.decrypt(password).unwrap();
        assert_eq!(key.p(), decrypted.p());
        assert_eq!(key.q(), decrypted.q());
    }

    #[test]
    fn test_decrypt_existing() {
        let password = "what";
        let encrypted =
            EncryptedKey::try_from(include_str!("../test_keys/encrypted_private_key.json"))
                .unwrap();
        encrypted.decrypt(password).unwrap();
    }
}
