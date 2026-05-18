use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use ssv_types::{msgid::MessageId, partial_sig::PartialSignatureKind};
use types::Hash256;

// ─── Plain helpers (non-serde) ───────────────────────────────────────────────

/// Decodes a hex string, stripping an optional `0x` prefix.
fn decode_hex<E: serde::de::Error>(hex_str: &str) -> Result<Vec<u8>, E> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    hex::decode(hex_str).map_err(|e| E::custom(format!("Failed to decode hex: {e}")))
}

/// Converts exactly 32 bytes into `Hash256`.
fn bytes_to_hash256<E: serde::de::Error>(bytes: &[u8]) -> Result<Hash256, E> {
    if bytes.len() != 32 {
        return Err(E::custom(format!(
            "expected 32 bytes for Hash256, got {}",
            bytes.len()
        )));
    }
    Ok(Hash256::from_slice(bytes))
}

// ─── Base64 ──────────────────────────────────────────────────────────────────

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

/// Deserializes a vector of base64 strings that may be `null` into `Vec<Vec<u8>>`,
/// treating `null` as an empty vector. Mirrors Go's marshaling of empty slices.
pub fn deserialize_base64_list_or_null<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Vec<u8>>, D::Error> {
    let opt: Option<Vec<String>> = Option::deserialize(deserializer)?;
    opt.unwrap_or_default()
        .into_iter()
        .map(|s| {
            STANDARD
                .decode(&s)
                .map_err(|e| serde::de::Error::custom(format!("Failed to decode base64: {e}")))
        })
        .collect()
}

/// Deserializes a base64-encoded string that may be `null` into `Option<Vec<u8>>`.
///
/// Error fixtures have `null` while success fixtures have a base64 string.
pub fn deserialize_base64_or_null<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<u8>>, D::Error> {
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            let bytes = STANDARD
                .decode(s)
                .map_err(|e| serde::de::Error::custom(format!("Failed to decode base64: {e}")))?;
            Ok(Some(bytes))
        }
    }
}

/// Deserializes a base64-encoded string, treating empty strings as empty `Vec<u8>`.
///
/// Some error fixtures have `"DataSSZ": ""` which is valid since it represents empty data
/// that should fail SSZ decoding.
pub fn deserialize_base64_or_empty<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(deserializer)?;
    if s.is_empty() {
        return Ok(Vec::new());
    }
    STANDARD
        .decode(s)
        .map_err(|e| serde::de::Error::custom(format!("Failed to decode base64: {e}")))
}

// ─── Hex ─────────────────────────────────────────────────────────────────────

/// Deserializes a hex string (with or without `0x` prefix) into `Vec<u8>`.
pub fn deserialize_hex<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    decode_hex::<D::Error>(&hex_str)
}

/// Deserializes an optional hex string (with or without `0x` prefix) into `Option<Vec<u8>>`.
pub fn deserialize_hex_option<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<u8>>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|s| decode_hex::<D::Error>(&s))
        .transpose()
}

/// Deserializes a hex string (without `0x` prefix) into `Hash256`.
///
/// The Go spec fixtures encode hash roots as 64-character hex strings representing 32 bytes.
pub fn deserialize_hex_hash256<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Hash256, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    let bytes = decode_hex::<D::Error>(&hex_str)?;
    bytes_to_hash256::<D::Error>(&bytes)
}

// ─── Hash256 ─────────────────────────────────────────────────────────────────

/// Deserializes a JSON byte array (array of 32 u8 values) into `Hash256`.
pub fn deserialize_bytes_to_hash256<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Hash256, D::Error> {
    let bytes = Vec::<u8>::deserialize(deserializer)?;
    bytes_to_hash256::<D::Error>(&bytes)
}

/// Deserializes an optional vector of byte arrays into `Option<Vec<Hash256>>`.
pub fn deserialize_hash256_list_option<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<Hash256>>, D::Error> {
    Option::<Vec<Vec<u8>>>::deserialize(deserializer)?
        .map(|byte_arrays| {
            byte_arrays
                .iter()
                .map(|bytes| bytes_to_hash256::<D::Error>(bytes))
                .collect()
        })
        .transpose()
}

// ─── MessageId ───────────────────────────────────────────────────────────────

/// Deserializes a hex string (with or without `0x` prefix) into a `MessageId`.
pub fn deserialize_hex_message_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<MessageId, D::Error> {
    let bytes = decode_hex::<D::Error>(&String::deserialize(deserializer)?)?;
    MessageId::try_from(bytes.as_slice()).map_err(|_| {
        serde::de::Error::custom(format!(
            "Invalid MessageId: expected 56 bytes, got {}",
            bytes.len()
        ))
    })
}

/// Deserializes a vector of hex strings into `Vec<MessageId>`.
pub fn deserialize_hex_message_id_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<MessageId>, D::Error> {
    Vec::<String>::deserialize(deserializer)?
        .iter()
        .map(|hex_str| {
            deserialize_hex_message_id(serde::de::value::StrDeserializer::<D::Error>::new(hex_str))
        })
        .collect()
}

// ─── Domain types ────────────────────────────────────────────────────────────

/// Deserializes a `u64` into `PartialSignatureKind`.
pub fn deserialize_partial_signature_kind<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<PartialSignatureKind, D::Error> {
    let value = u64::deserialize(deserializer)?;
    PartialSignatureKind::try_from(value).map_err(|_| {
        serde::de::Error::custom(format!("Invalid PartialSignatureKind value: {value}"))
    })
}
