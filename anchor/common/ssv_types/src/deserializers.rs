use base64::prelude::*;
use serde::{Deserialize, Deserializer, de::Error};
use serde_json::Value;
use ssz_types::VariableList;
use types::{Hash256, Slot};

use crate::{
    ValidatorIndex,
    message::{SSVMessageDataLen, SignatureList},
    msgid::MessageId,
    partial_sig::PartialSignatureKind,
    try_to_variable_list,
};

pub fn deserialize_base64_or_empty<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: TryFrom<Vec<u8>>,
{
    let value = Value::deserialize(deserializer)?;

    match value {
        Value::Null => Ok(Vec::new()), // Return empty Vec for null values
        Value::String(s) => BASE64_STANDARD
            .decode(s.as_bytes())
            .map_err(D::Error::custom),
        _ => Err(D::Error::custom("Expected null or a base64 string")),
    }
    .and_then(|vec| {
        vec.try_into()
            .map_err(|_| D::Error::custom("Failed to convert from Vec<u8> to actual type"))
    })
}

pub fn deserialize_base64_signatures<'de, D>(deserializer: D) -> Result<SignatureList, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string_vec: Vec<String> = serde::Deserialize::deserialize(deserializer)?;

    let mut signatures = VariableList::empty();

    for string in string_vec {
        let decoded_bytes = BASE64_STANDARD
            .decode(&string)
            .map_err(serde::de::Error::custom)?;

        let signature_variable_list = VariableList::new(decoded_bytes)
            .map_err(|e| D::Error::custom(format!("Signature too long: {e:?}")))?;

        if let Err(err) = signatures.push(signature_variable_list) {
            return Err(D::Error::custom(format!("Too many signatures: {err:?}")));
        }
    }

    Ok(signatures)
}

pub fn deserialize_base64_message_data<'de, D>(
    deserializer: D,
) -> Result<VariableList<u8, SSVMessageDataLen>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;

    match value {
        Value::Null => {
            Ok(
                try_to_variable_list(vec![], |_, _| D::Error::custom("Empty vec too large"))
                    .unwrap(),
            )
        } // Empty vec always fits
        Value::String(s) => {
            let decoded = BASE64_STANDARD
                .decode(s.as_bytes())
                .map_err(D::Error::custom)?;
            try_to_variable_list(decoded, |actual, max| {
                D::Error::custom(format!(
                    "Data too large for VariableList: {} > {}",
                    actual, max
                ))
            })
        }
        _ => Err(D::Error::custom("Expected null or a base64 string")),
    }
}

pub fn deserialize_hex_message_id<'de, D>(deserializer: D) -> Result<MessageId, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes =
        hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;

    if bytes.len() != 56 {
        return Err(Error::custom(format!(
            "Expected 56 bytes for MessageId, got {}",
            bytes.len()
        )));
    }

    let array: [u8; 56] = bytes
        .try_into()
        .map_err(|_| Error::custom("Failed to convert to array"))?;
    Ok(MessageId::from(array))
}

pub fn deserialize_slot<'de, D>(deserializer: D) -> Result<Slot, D::Error>
where
    D: Deserializer<'de>,
{
    let slot_str = String::deserialize(deserializer)?;
    slot_str
        .parse::<u64>()
        .map(Slot::new)
        .map_err(|e| Error::custom(format!("Failed to parse slot: {e}")))
}

pub fn deserialize_partial_signature_kind<'de, D>(
    deserializer: D,
) -> Result<PartialSignatureKind, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value > 5 {
        return Err(Error::custom(format!(
            "Invalid PartialSignatureKind value: {}",
            value
        )));
    }
    Ok(PartialSignatureKind::from(value))
}

pub fn deserialize_signature<'de, D>(deserializer: D) -> Result<types::Signature, D::Error>
where
    D: Deserializer<'de>,
{
    let sig_opt: Option<String> = Option::deserialize(deserializer)?;
    match sig_opt {
        Some(sig_str) => {
            // Handle empty string as empty signature (for invalid test cases)
            if sig_str.is_empty() {
                return Ok(types::Signature::empty());
            }

            let sig_bytes = if let Some(stripped) = sig_str.strip_prefix("0x") {
                // Handle hex string with 0x prefix
                hex::decode(stripped)
                    .map_err(|e| Error::custom(format!("Failed to decode hex signature: {e}")))?
            } else if sig_str.chars().all(|c| c.is_ascii_hexdigit()) && sig_str.len() % 2 == 0 {
                // Try hex without prefix if all characters are hex digits and even length
                hex::decode(&sig_str)
                    .map_err(|e| Error::custom(format!("Failed to decode hex signature: {e}")))?
            } else {
                // Fall back to base64 for backward compatibility
                BASE64_STANDARD
                    .decode(&sig_str)
                    .map_err(|e| Error::custom(format!("Failed to decode base64 signature: {e}")))?
            };

            if sig_bytes.len() != 96 {
                return Err(Error::custom(format!(
                    "Signature must be 96 bytes, got {}",
                    sig_bytes.len()
                )));
            }

            Ok(types::Signature::deserialize(&sig_bytes)
                .map_err(|e| Error::custom(format!("Failed to parse signature: {e:?}")))?)
        }
        None => {
            // Return empty signature for null values
            Ok(types::Signature::empty())
        }
    }
}

pub fn deserialize_hash256<'de, D>(deserializer: D) -> Result<Hash256, D::Error>
where
    D: Deserializer<'de>,
{
    let hash_str = String::deserialize(deserializer)?;
    let hash_str = hash_str.strip_prefix("0x").unwrap_or(&hash_str);
    let bytes =
        hex::decode(hash_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(Error::custom(format!(
            "Expected 32 bytes for Hash256, got {}",
            bytes.len()
        )));
    }
    Ok(Hash256::from_slice(&bytes))
}

pub fn deserialize_validator_index<'de, D>(deserializer: D) -> Result<ValidatorIndex, D::Error>
where
    D: Deserializer<'de>,
{
    let index_str = String::deserialize(deserializer)?;
    index_str
        .parse::<usize>()
        .map(ValidatorIndex)
        .map_err(|e| Error::custom(format!("Failed to parse validator index: {e}")))
}
