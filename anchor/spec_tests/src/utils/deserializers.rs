use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, de::Error};
use ssv_types::{consensus::QbftMessageType, msgid::MessageId};
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

/// Deserialize an optional base64 string to optional bytes
pub fn deserialize_base64_option<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        Some(s) => STANDARD
            .decode(&s)
            .map(Some)
            .map_err(|e| Error::custom(format!("Failed to decode base64: {e}"))),
        None => Ok(None),
    }
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

/// Deserialize a hex string (with or without 0x prefix) to bytes
pub fn deserialize_hex<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))
}

/// Deserialize a hex string to Hash256
pub fn deserialize_hex_hash256<'de, D>(deserializer: D) -> Result<Hash256, D::Error>
where
    D: Deserializer<'de>,
{
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

/// Deserialize an optional Hash256 from hex string
pub fn deserialize_hex_hash256_option<'de, D>(deserializer: D) -> Result<Option<Hash256>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        Some(hex_str) if !hex_str.is_empty() => {
            let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
            let bytes = hex::decode(hex_str)
                .map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;

            if bytes.len() != 32 {
                return Err(Error::custom(format!(
                    "Expected 32 bytes for Hash256, got {}",
                    bytes.len()
                )));
            }

            Ok(Some(Hash256::from_slice(&bytes)))
        }
        Some(_) | None => Ok(None),
    }
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

/// Deserialize MessageId from hex string
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

/// Deserialize QBFT message type from string-based CreateType
pub fn deserialize_create_type<'de, D>(deserializer: D) -> Result<QbftMessageType, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;

    match value.as_str() {
        "CreateProposal" | "createProposal" => Ok(QbftMessageType::Proposal),
        "CreatePrepare" | "createPrepare" => Ok(QbftMessageType::Prepare),
        "CreateCommit" | "createCommit" => Ok(QbftMessageType::Commit),
        "CreateRoundChange" | "createRoundChange" => Ok(QbftMessageType::RoundChange),
        _ => Err(D::Error::custom(format!(
            "Invalid CreateType value: {}",
            value
        ))),
    }
}
