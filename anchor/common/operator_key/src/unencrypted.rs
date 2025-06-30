//! The unencrypted private key format.
//!
//! This is simply the base64 encoded PKCS1 private key. This is the format as supported by the
//! go-ssv config file.
use base64::prelude::*;
use openssl::{
    pkey::{HasPrivate, Private},
    rsa::Rsa,
};

use crate::ConversionError;

pub fn from_base64(pem_data: &[u8]) -> Result<Rsa<Private>, ConversionError> {
    let pem_decoded = BASE64_STANDARD.decode(pem_data)?;
    let rsa_key = Rsa::private_key_from_pem(&pem_decoded)?;
    Ok(rsa_key)
}

pub fn to_base64<T: HasPrivate>(key: &Rsa<T>) -> Result<String, ConversionError> {
    let pem = key.private_key_to_pem()?;
    Ok(BASE64_STANDARD.encode(pem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversion() {
        let key = Rsa::generate(2048).unwrap();
        let string = to_base64(&key).unwrap();
        let deserialized = from_base64(string.as_bytes()).unwrap();
        assert_eq!(key.p(), deserialized.p());
        assert_eq!(key.q(), deserialized.q());
    }
}
