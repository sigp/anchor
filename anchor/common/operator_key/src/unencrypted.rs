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
