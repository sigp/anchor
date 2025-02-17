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
pub fn extract_key(keystore: &Keystore, password: &str) -> SecretKey {
    let kdf_params = &keystore.crypto.kdf.params;
    let salt = hex::decode(&kdf_params.salt).unwrap();

    let scrypt_params = ScryptParams::new(
        (kdf_params.n as f64).log2() as u8,
        kdf_params.r,
        kdf_params.p,
        salt.len(),
    )
    .unwrap();

    let mut derived_key = vec![0u8; kdf_params.dklen as usize];
    scrypt(password.as_ref(), &salt, &scrypt_params, &mut derived_key).unwrap();

    let decryptor = Aes128Ctr::new(&derived_key[..16], &keystore.crypto.cipher.params.iv[..16])
        .expect("invalid length");

    let mut pk = keystore.crypto.cipher.message.clone();
    decryptor.apply_keystream(&mut pk);
    SecretKey::deserialize(pk.as_slice()).unwrap()
}

// Encrypt the keyshare with the operators public kye
pub fn encrypt_keyshares(key_shares: Vec<KeyShare>) -> Result<Vec<EncryptedKeyShare>, KeygenError> {
    Ok(key_shares
        .into_iter()
        .map(|share| {
            let pkey = PKey::from_rsa(share.public_key.clone()).unwrap();
            let encrypter = Encrypter::new(&pkey).unwrap();

            let data = share.keyshare.serialize();
            let data = data.as_bytes();

            let buffer_len = encrypter.encrypt_len(data).unwrap();
            let mut encrypted = vec![0; buffer_len];

            // Encrypt and truncate the buffer
            let encrypted_len = encrypter.encrypt(data, &mut encrypted).unwrap();
            encrypted.truncate(encrypted_len);

            EncryptedKeyShare {
                id: share.id,
                public_key: share.public_key,
                encrypted_keyshare: encrypted,
            }
        })
        .collect())
}
