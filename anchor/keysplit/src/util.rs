use std::str::FromStr;

use base64::prelude::*;
use hex::FromHex;
use openssl::{pkey::Public, rsa::Rsa};
use serde::{Deserialize, Deserializer, Serializer};
use types::Address;

// Serde deserialization and serialization helper functions
pub(crate) fn hex_to_buffer<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    String::deserialize(deserializer)
        .and_then(|string| Vec::from_hex(&string).map_err(|err| Error::custom(err.to_string())))
}

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
