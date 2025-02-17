use crate::{base_processing, KeygenError, Manual, OutputData, EncryptedKeyShare};
use openssl::encrypt::Encrypter;
use openssl::pkey::PKey;
use openssl::pkey::Public;
use openssl::rsa::Rsa;


pub fn manual_split(manual: Manual) -> Result<Vec<EncryptedKeyShare>, KeygenError> {
    let mut encrypted_keys = Vec::new();

    let validator_keys = base_processing(&manual.shared)?;

    for (share, key) in validator_keys.iter().zip(manual.public_keys) {
        let pkey = PKey::from_rsa(key.clone()).unwrap();
        let encrypter = Encrypter::new(&pkey).unwrap();

        let data = share.keyshare.serialize();
        let data = data.as_bytes();

        let buffer_len = encrypter.encrypt_len(data).unwrap();
        let mut encrypted = vec![0; buffer_len];

        // Encrypt and truncate the buffer
        let encrypted_len = encrypter.encrypt(data, &mut encrypted).unwrap();
        encrypted.truncate(encrypted_len);

        let encrypted_key = EncryptedKeyShare {
            public_key: key,
            encrypted_key: encrypted,
        };
        encrypted_keys.push(encrypted_key);
    }

    Ok(encrypted_keys)
}
