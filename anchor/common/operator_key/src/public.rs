use base64::prelude::*;
use openssl::{
    pkey::{HasPublic, Public},
    rsa::Rsa,
};

use crate::ConversionError;

const PKCS1_HEADER: &str = "-----BEGIN RSA PUBLIC KEY-----";
const PKCS8_HEADER: &str = "-----BEGIN PUBLIC KEY-----";

const PKCS1_FOOTER: &str = "-----END RSA PUBLIC KEY-----";
const PKCS8_FOOTER: &str = "-----END PUBLIC KEY-----";

// Parse from a RSA public key string into the associated RSA representation
pub fn from_base64(pem_data: &[u8]) -> Result<Rsa<Public>, ConversionError> {
    // First decode the base64 data
    let pem_decoded = BASE64_STANDARD.decode(pem_data)?;

    // Convert the decoded data to a string
    let mut pem_string = String::from_utf8(pem_decoded).map_err(|err| err.utf8_error())?;

    // Fix the header - replace PKCS1 header with PKCS8 header
    pem_string = pem_string
        .replace(PKCS1_HEADER, PKCS8_HEADER)
        .replace(PKCS1_FOOTER, PKCS8_FOOTER);

    // Parse the PEM string into an RSA public key using PKCS8 format
    let rsa_pubkey = Rsa::public_key_from_pem(pem_string.as_bytes())?;

    Ok(rsa_pubkey)
}

pub fn to_base64<T: HasPublic>(key: &Rsa<T>) -> Result<String, ConversionError> {
    let pem_string = String::from_utf8(key.public_key_to_pem()?).map_err(|err| err.utf8_error())?;

    let pem_string = pem_string
        .replace(PKCS8_HEADER, PKCS1_HEADER)
        .replace(PKCS8_FOOTER, PKCS1_FOOTER);

    Ok(BASE64_STANDARD.encode(pem_string))
}
