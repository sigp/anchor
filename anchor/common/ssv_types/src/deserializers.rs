//! Custom serde deserializers for Go JSON fixture format.
//!
//! These deserializers bridge Go's JSON serialization format to Anchor's Rust types.
//! Only compiled when the `serde` feature is enabled.
//!
//! Referenced via `#[serde(deserialize_with = "...")]` on struct fields.

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, de::Error};
use ssz_types::VariableList;
use types::{Hash256, Slot};

use crate::{
    ValidatorIndex, message::SSVMessageDataLen, msgid::MessageId, partial_sig::PartialSignatureKind,
};

pub fn deserialize_hex_message_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<MessageId, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes =
        hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;
    MessageId::try_from(bytes.as_slice()).map_err(|_| {
        Error::custom(format!(
            "Invalid MessageId: expected 56 bytes, got {}",
            bytes.len()
        ))
    })
}

pub fn deserialize_base64_message_data<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<VariableList<u8, SSVMessageDataLen>, D::Error> {
    let b64_str = String::deserialize(deserializer)?;
    let bytes = STANDARD
        .decode(&b64_str)
        .map_err(|e| Error::custom(format!("Failed to decode base64 data: {e}")))?;
    VariableList::new(bytes).map_err(|_| Error::custom("SSVMessage data exceeds maximum length"))
}

pub fn deserialize_partial_signature_kind<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<PartialSignatureKind, D::Error> {
    let value = u64::deserialize(deserializer)?;
    PartialSignatureKind::try_from(value)
        .map_err(|_| Error::custom(format!("Invalid PartialSignatureKind value: {value}")))
}

pub fn deserialize_slot<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Slot, D::Error> {
    let slot_str = String::deserialize(deserializer)?;
    slot_str
        .parse::<u64>()
        .map(Slot::new)
        .map_err(|e| Error::custom(format!("Failed to parse slot: {e}")))
}

pub fn deserialize_signature<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<bls::Signature, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes =
        hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;
    bls::Signature::deserialize(&bytes)
        .map_err(|e| Error::custom(format!("Invalid BLS signature: {e:?}")))
}

pub fn deserialize_hash256<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Hash256, D::Error> {
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes =
        hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(Error::custom(format!(
            "Expected 32 bytes for Hash256, got {}",
            bytes.len()
        )));
    }
    Ok(Hash256::from_slice(&bytes))
}

pub fn deserialize_validator_index<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ValidatorIndex, D::Error> {
    let index_str = String::deserialize(deserializer)?;
    index_str
        .parse::<usize>()
        .map(ValidatorIndex)
        .map_err(|e| Error::custom(format!("Failed to parse validator index: {e}")))
}
