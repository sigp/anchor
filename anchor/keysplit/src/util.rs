use std::{fs, path::Path, str::FromStr};

use base64::prelude::*;
use openssl::{pkey::Public, rsa::Rsa};
use serde::Serializer;
use types::Address;
use zeroize::Zeroizing;

// Serde deserialization and serialization helper functions
pub(crate) fn parse_address(s: &str) -> Result<Address, String> {
    Address::from_str(s).map_err(|e| e.to_string())
}

pub(crate) fn serialize_rsa<S>(key: &Rsa<Public>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let serialized_key = key.public_key_to_pem().map_err(serde::ser::Error::custom)?;

    // Convert the decoded data to a string
    let mut pem_string = String::from_utf8(serialized_key).map_err(serde::ser::Error::custom)?;

    // Fix the header - replace PKCS8 header with PKCS1 header
    pem_string = pem_string
        .replace(
            "-----BEGIN PUBLIC KEY-----",
            "-----BEGIN RSA PUBLIC KEY-----",
        )
        .replace("-----END PUBLIC KEY-----", "-----END RSA PUBLIC KEY-----");

    let encoded = BASE64_STANDARD.encode(pem_string.clone());
    s.serialize_str(&encoded)
}

pub(crate) fn read_password(file: Option<&Path>) -> Result<Zeroizing<String>, String> {
    if let Some(path) = file {
        let full = Zeroizing::new(
            fs::read_to_string(path).map_err(|e| format!("Unable to read password file: {e}"))?,
        );
        Ok(Zeroizing::new(full.trim_matches(['\n', '\r']).to_string()))
    } else {
        let password = rpassword::prompt_password("Enter keystore password: ")
            .map_err(|e| format!("Unable to read password from stdin: {e}"))?;
        Ok(Zeroizing::new(password))
    }
}
