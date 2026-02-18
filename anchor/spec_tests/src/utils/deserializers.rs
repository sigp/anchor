use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use ssv_types::msgid::MessageId;
use types::Hash256;

/// Deserializes a base64-encoded string into `Vec<u8>`.
pub fn deserialize_base64<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(deserializer)?;
    STANDARD
        .decode(s)
        .map_err(|e| serde::de::Error::custom(format!("Failed to decode base64: {e}")))
}

/// Deserializes a vector of base64 strings into `Vec<Vec<u8>>`.
pub fn deserialize_base64_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Vec<u8>>, D::Error> {
    let strings: Vec<String> = Vec::deserialize(deserializer)?;
    strings
        .into_iter()
        .map(|s| {
            STANDARD
                .decode(&s)
                .map_err(|e| serde::de::Error::custom(format!("Failed to decode base64: {e}")))
        })
        .collect()
}

/// Deserializes an optional vector of base64 strings into `Option<Vec<Vec<u8>>>`.
pub fn deserialize_base64_list_option<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<Vec<u8>>>, D::Error> {
    let opt: Option<Vec<String>> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(strings) => {
            let result: Result<Vec<Vec<u8>>, _> = strings
                .into_iter()
                .map(|s| {
                    STANDARD.decode(&s).map_err(|e| {
                        serde::de::Error::custom(format!("Failed to decode base64: {e}"))
                    })
                })
                .collect();
            result.map(Some)
        }
    }
}

/// Deserializes an optional hex string (with or without `0x` prefix) into `Option<Vec<u8>>`.
pub fn deserialize_hex_option<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<u8>>, D::Error> {
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(hex_str) => {
            let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
            hex::decode(hex_str)
                .map(Some)
                .map_err(|e| serde::de::Error::custom(format!("Failed to decode hex: {e}")))
        }
    }
}

/// Deserializes a JSON byte array (array of 32 u8 values) into `Hash256`.
pub fn deserialize_bytes_to_hash256<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Hash256, D::Error> {
    let bytes = Vec::<u8>::deserialize(deserializer)?;
    if bytes.len() != 32 {
        return Err(serde::de::Error::custom(format!(
            "expected 32 bytes for Hash256, got {}",
            bytes.len()
        )));
    }
    Ok(Hash256::from_slice(&bytes))
}

/// Deserializes an optional vector of byte arrays into `Option<Vec<Hash256>>`.
pub fn deserialize_hash256_list_option<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<Hash256>>, D::Error> {
    let opt: Option<Vec<Vec<u8>>> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(byte_arrays) => {
            let result: Result<Vec<Hash256>, _> = byte_arrays
                .into_iter()
                .map(|bytes| {
                    if bytes.len() != 32 {
                        return Err(serde::de::Error::custom(format!(
                            "expected 32 bytes for Hash256, got {}",
                            bytes.len()
                        )));
                    }
                    Ok(Hash256::from_slice(&bytes))
                })
                .collect();
            result.map(Some)
        }
    }
}

/// Deserializes a vector of hex strings into `Vec<MessageId>`.
///
/// Each hex string is decoded to 56 bytes and converted to a `MessageId`.
pub fn deserialize_hex_message_id_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<MessageId>, D::Error> {
    let hex_strings: Vec<String> = Vec::deserialize(deserializer)?;
    hex_strings
        .into_iter()
        .map(|hex_str| {
            let bytes = hex::decode(&hex_str)
                .map_err(|e| serde::de::Error::custom(format!("Failed to decode hex: {e}")))?;
            MessageId::try_from(bytes.as_slice()).map_err(|_| {
                serde::de::Error::custom(format!(
                    "Invalid MessageId: expected 56 bytes, got {}",
                    bytes.len()
                ))
            })
        })
        .collect()
}
