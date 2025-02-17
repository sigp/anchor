use base64::prelude::*;
use hex::{FromHex, ToHex};
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use serde::{Deserialize, Deserializer, Serializer};
use std::str::FromStr;
use types::Address;

pub(crate) fn buffer_to_hex<T, S>(buffer: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: AsRef<[u8]>,
    S: Serializer,
{
    serializer.serialize_str(&buffer.encode_hex::<String>())
}

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
    let encoded = BASE64_STANDARD.encode(serialized_key.clone());
    s.serialize_str(&encoded)
}
