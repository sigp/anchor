use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, de::Error};
use ssv_types::{deserializers::deserialize_hex_message_id, msgid::MessageId};
use types::Hash256;

/// Deserialize a base64 string to bytes
pub fn deserialize_base64<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let base64_string = String::deserialize(deserializer)?;
    STANDARD
        .decode(&base64_string)
        .map_err(|e| Error::custom(format!("Failed to decode base64: {e}")))
}

/// Deserialize a vector of base64 strings to vector of byte arrays
pub fn deserialize_base64_list<'de, D>(deserializer: D) -> Result<Vec<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    let strings: Vec<String> = Vec::deserialize(deserializer)?;
    let mut result = Vec::with_capacity(strings.len());
    for s in strings {
        let bytes = STANDARD
            .decode(&s)
            .map_err(|e| Error::custom(format!("Failed to decode base64: {e}")))?;
        result.push(bytes);
    }
    Ok(result)
}

/// Deserialize an optional vector of base64 strings to optional vector of byte arrays
pub fn deserialize_base64_list_option<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Vec<u8>>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<String>> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(strings) => {
            let mut result = Vec::with_capacity(strings.len());
            for s in strings {
                let bytes = STANDARD
                    .decode(&s)
                    .map_err(|e| Error::custom(format!("Failed to decode base64: {e}")))?;
                result.push(bytes);
            }
            Ok(Some(result))
        }
    }
}

/// Deserialize an optional hex string to optional bytes
pub fn deserialize_hex_option<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(hex_str) => {
            let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
            hex::decode(hex_str)
                .map(Some)
                .map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))
        }
    }
}

/// Convert byte array to Hash256
pub fn deserialize_bytes_to_hash256<'de, D>(deserializer: D) -> Result<Hash256, D::Error>
where
    D: Deserializer<'de>,
{
    let bytes = <Vec<u8>>::deserialize(deserializer)?;
    if bytes.len() != 32 {
        return Err(Error::custom(format!(
            "Expected 32 bytes for Hash256, got {}",
            bytes.len()
        )));
    }
    Ok(Hash256::from_slice(&bytes))
}

/// Deserialize optional vector of Hash256 from byte arrays
pub fn deserialize_hash256_list_option<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Hash256>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<Vec<u8>>> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(byte_arrays) => {
            let mut result = Vec::with_capacity(byte_arrays.len());
            for bytes in byte_arrays {
                if bytes.len() != 32 {
                    return Err(Error::custom(format!(
                        "Expected 32 bytes for Hash256, got {}",
                        bytes.len()
                    )));
                }
                result.push(Hash256::from_slice(&bytes));
            }
            Ok(Some(result))
        }
    }
}

/// Deserialize vector of MessageIds from hex strings
pub fn deserialize_hex_message_id_list<'de, D>(deserializer: D) -> Result<Vec<MessageId>, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_strings: Vec<String> = Vec::deserialize(deserializer)?;
    let mut result = Vec::with_capacity(hex_strings.len());

    for hex_str in hex_strings {
        result.push(deserialize_hex_message_id(
            serde::de::value::StrDeserializer::<D::Error>::new(&hex_str),
        )?);
    }

    Ok(result)
}
