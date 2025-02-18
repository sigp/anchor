use crate::keystore::Keystore;
use crate::EncryptedKeyShare;
use crate::KeyShare;
use crate::KeygenError;
use aes::cipher::InnerIvInit;
use aes::cipher::KeyInit;
use aes::cipher::StreamCipherCore;
use aes::Aes128;
use ctr::cipher;
use openssl::encrypt::Encrypter;
use openssl::pkey::PKey;
use scrypt::{scrypt, Params as ScryptParams};
use types::SecretKey;

struct Aes128Ctr {
    inner: ctr::CtrCore<Aes128, ctr::flavors::Ctr128BE>,
}

impl Aes128Ctr {
    fn new(key: &[u8], iv: &[u8]) -> Result<Self, cipher::InvalidLength> {
        let cipher = aes::Aes128::new_from_slice(key).unwrap();
        let inner = ctr::CtrCore::inner_iv_slice_init(cipher, iv).unwrap();
        Ok(Self { inner })
    }

    fn apply_keystream(self, buf: &mut [u8]) {
        self.inner.apply_keystream_partial(buf.into());
    }
}

// From the keystore file, extract the decrypted validator keys
pub fn extract_key(keystore: &Keystore, password: &str) -> Result<SecretKey, KeygenError> {
    let kdf_params = &keystore.crypto.kdf.params;
    let salt = hex::decode(&kdf_params.salt)
        .map_err(|e| KeygenError::Misc(format!("Failed to decode salt: {e}")))?;

    let scrypt_params = ScryptParams::new(
        (kdf_params.n as f64).log2() as u8,
        kdf_params.r,
        kdf_params.p,
        salt.len(),
    )
    .map_err(|e| KeygenError::Scrypt(format!("Failed to construct scrypt params: {e}")))?;

    let mut derived_key = vec![0u8; kdf_params.dklen as usize];
    scrypt(password.as_ref(), &salt, &scrypt_params, &mut derived_key)
        .map_err(|e| KeygenError::Scrypt(format!("Faild to run key derivation function: {e}")))?;

    let decryptor = Aes128Ctr::new(&derived_key[..16], &keystore.crypto.cipher.params.iv[..16])
        .expect("invalid length");

    let mut pk = keystore.crypto.cipher.message.clone();
    decryptor.apply_keystream(&mut pk);

    let deser_pk = SecretKey::deserialize(pk.as_slice())
        .map_err(|e| KeygenError::Misc(format!("Failed to deserialize secret key: {:?}", e)))?;
    Ok(deser_pk)
}

// Encrypt the keyshare with the operators public kye
pub fn encrypt_keyshares(key_shares: Vec<KeyShare>) -> Result<Vec<EncryptedKeyShare>, KeygenError> {
    key_shares
        .into_iter()
        .map(|share| {
            let pkey = PKey::from_rsa(share.public_key.clone())
                .map_err(|e| KeygenError::Misc(format!("Failed to map from rsa to pkey: {e}")))?;
            let encrypter = Encrypter::new(&pkey).map_err(|e| {
                KeygenError::Misc(format!("Failed to construct encrypter with pkey: {e}"))
            })?;
            let data = share.keyshare.serialize();
            let data = data.as_bytes();
            let buffer_len = encrypter
                .encrypt_len(data)
                .map_err(|e| KeygenError::Misc(format!("Failed to set encryption length: {e}")))?;
            let mut encrypted = vec![0; buffer_len];
            // Encrypt and truncate the buffer
            let encrypted_len = encrypter
                .encrypt(data, &mut encrypted)
                .map_err(|e| KeygenError::Misc(format!("Failed to perform encryption: {e}")))?;
            encrypted.truncate(encrypted_len);
            Ok(EncryptedKeyShare {
                id: share.id,
                public_key: share.public_key,
                encrypted_keyshare: encrypted,
            })
        })
        .collect()
}
