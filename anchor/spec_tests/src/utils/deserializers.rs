use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use types::Hash256;

/// Deserializes a base64-encoded string into `Vec<u8>`.
///
/// Expects a JSON string field containing standard base64-encoded data.
pub fn deserialize_base64<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(deserializer)?;
    STANDARD
        .decode(s)
        .map_err(|e| serde::de::Error::custom(format!("Failed to decode base64: {e}")))
}

/// Deserializes a JSON byte array (array of 32 u8 values) into `Hash256`.
///
/// Expects a JSON array field containing exactly 32 unsigned integer values.
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
