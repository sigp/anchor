use crate::keystore::Keystore;
use aes::cipher::InnerIvInit;
use aes::cipher::KeyInit;
use aes::cipher::StreamCipherCore;
use aes::Aes128;
use ctr::cipher;
use scrypt::{scrypt, Params as ScryptParams};
use types::{PublicKey, SecretKey};

// Validator keypair extracted from the keystore file
pub struct ValidatorKeys {
    pub public_key: PublicKey,
    pub secret_key: SecretKey,
}

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
pub fn extract_keys(keystore: &Keystore, password: &str) -> ValidatorKeys {
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
    let sk = SecretKey::deserialize(pk.as_slice()).unwrap();

    ValidatorKeys {
        public_key: sk.public_key(),
        secret_key: sk,
    }
}
