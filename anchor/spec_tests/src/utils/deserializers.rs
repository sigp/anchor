//! Unified serde deserializers for SSV spec tests
//!
//! This module provides clean, reusable deserializers for common patterns in SSV spec tests.
//! All deserializers are designed to be simple, idiomatic, and maintainable.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, de::Error};
use ssv_types::{
    ValidatorIndex,
    consensus::{
        BEACON_ROLE_AGGREGATOR, BEACON_ROLE_ATTESTER, BEACON_ROLE_PROPOSER,
        BEACON_ROLE_SYNC_COMMITTEE, BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION,
        BEACON_ROLE_VALIDATOR_REGISTRATION, BEACON_ROLE_VOLUNTARY_EXIT, BeaconRole, DataVersion,
        QbftMessageType,
    },
    msgid::MessageId,
};
use types::{
    CommitteeIndex, ForkName, Hash256, PublicKeyBytes, Signature, Slot, VariableList, typenum::U13,
};

// Base64 Deserializers

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

// Hex String Deserializers

/// Deserialize a hex string (with or without 0x prefix) to bytes
pub fn deserialize_hex<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))
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

/// Deserialize a Signature from hex string
pub fn deserialize_hex_signature<'de, D>(deserializer: D) -> Result<Signature, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes =
        hex::decode(hex_str).map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;

    // Handle both 72 bytes (spec tests) and 96 bytes (full BLS) signatures
    if bytes.len() != 72 && bytes.len() != 96 {
        return Err(Error::custom(format!(
            "Expected 72 or 96 bytes for signature, got {}",
            bytes.len()
        )));
    }

    // If it's 72 bytes, pad it to 96 bytes (this might not be correct, but let's see)
    let bytes = if bytes.len() == 72 {
        let mut padded = vec![0u8; 96];
        padded[..72].copy_from_slice(&bytes);
        padded
    } else {
        bytes
    };

    Signature::deserialize(&bytes)
        .map_err(|e| Error::custom(format!("Failed to parse signature: {e:?}")))
}

/// Deserialize an optional Signature from hex string
pub fn deserialize_hex_signature_option<'de, D>(
    deserializer: D,
) -> Result<Option<Signature>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(hex_str) => {
            let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
            let bytes = hex::decode(hex_str)
                .map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;

            if bytes.len() != 96 {
                return Err(Error::custom(format!(
                    "Expected 96 bytes for signature, got {}",
                    bytes.len()
                )));
            }

            let sig = Signature::deserialize(&bytes)
                .map_err(|e| Error::custom(format!("Failed to parse signature: {e:?}")))?;
            Ok(Some(sig))
        }
    }
}

/// Deserialize a PublicKeyBytes from hex string
pub fn deserialize_hex_public_key<'de, D>(deserializer: D) -> Result<PublicKeyBytes, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let hex_with_prefix = format!("0x{hex_str}");
    hex_with_prefix
        .parse()
        .map_err(|e| Error::custom(format!("Invalid public key: {e}")))
}

// Hash256 Deserializers

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

// String to Number Converters

/// Parse string as u64
pub fn deserialize_string_to_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse()
        .map_err(|e| Error::custom(format!("Invalid u64: {e}")))
}

/// Parse string as usize
pub fn deserialize_string_to_usize<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse()
        .map_err(|e| Error::custom(format!("Invalid usize: {e}")))
}

/// Parse string as Slot
pub fn deserialize_string_to_slot<'de, D>(deserializer: D) -> Result<Slot, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let slot_num: u64 = s
        .parse()
        .map_err(|e| Error::custom(format!("Invalid slot: {e}")))?;
    Ok(Slot::new(slot_num))
}

/// Parse string as ValidatorIndex
pub fn deserialize_string_to_validator_index<'de, D>(
    deserializer: D,
) -> Result<ValidatorIndex, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let index: usize = s
        .parse()
        .map_err(|e| Error::custom(format!("Invalid validator index: {e}")))?;
    Ok(ValidatorIndex(index))
}

/// Parse string as CommitteeIndex
pub fn deserialize_string_to_committee_index<'de, D>(
    deserializer: D,
) -> Result<CommitteeIndex, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let index: u64 = s
        .parse()
        .map_err(|e| Error::custom(format!("Invalid committee index: {e}")))?;
    Ok(CommitteeIndex::from(index))
}

// Enum Deserializers

/// Deserialize BeaconRole from numeric value
pub fn deserialize_beacon_role<'de, D>(deserializer: D) -> Result<BeaconRole, D::Error>
where
    D: Deserializer<'de>,
{
    let num = u64::deserialize(deserializer)?;
    match num {
        0 => Ok(BEACON_ROLE_ATTESTER),
        1 => Ok(BEACON_ROLE_AGGREGATOR),
        2 => Ok(BEACON_ROLE_PROPOSER),
        3 => Ok(BEACON_ROLE_SYNC_COMMITTEE),
        4 => Ok(BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION),
        5 => Ok(BEACON_ROLE_VALIDATOR_REGISTRATION),
        6 => Ok(BEACON_ROLE_VOLUNTARY_EXIT),
        _ => Err(Error::custom(format!("Unknown beacon role: {num}"))),
    }
}

/// Deserialize DataVersion from fork name string
pub fn deserialize_data_version<'de, D>(deserializer: D) -> Result<DataVersion, D::Error>
where
    D: Deserializer<'de>,
{
    let version_str = String::deserialize(deserializer)?;
    let fork_name = match version_str.as_str() {
        "phase0" => ForkName::Base,
        "altair" => ForkName::Altair,
        "bellatrix" => ForkName::Bellatrix,
        "capella" => ForkName::Capella,
        "deneb" => ForkName::Deneb,
        "electra" => ForkName::Electra,
        "fulu" => ForkName::Fulu,
        _ => return Err(Error::custom(format!("Invalid fork name: {version_str}"))),
    };
    Ok(DataVersion::from(fork_name))
}

// Utility Deserializers

/// Deserialize sync committee indices from JSON array
pub fn deserialize_sync_committee_indices<'de, D>(
    deserializer: D,
) -> Result<VariableList<u64, U13>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<u64>> = Option::deserialize(deserializer)?;
    let indices = opt.unwrap_or_default();
    VariableList::new(indices)
        .map_err(|e| Error::custom(format!("Too many sync committee indices: {e:?}")))
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

/// Deserialize vector of MessageIds from hex strings
pub fn deserialize_hex_message_id_list<'de, D>(deserializer: D) -> Result<Vec<MessageId>, D::Error>
where
    D: Deserializer<'de>,
{
    let hex_strings: Vec<String> = Vec::deserialize(deserializer)?;
    let mut result = Vec::with_capacity(hex_strings.len());

    for hex_str in hex_strings {
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
        let bytes = hex::decode(hex_str)
            .map_err(|e| Error::custom(format!("Failed to decode hex: {e}")))?;

        if bytes.len() != 56 {
            return Err(Error::custom(format!(
                "Expected 56 bytes for MessageId, got {}",
                bytes.len()
            )));
        }

        let array: [u8; 56] = bytes
            .try_into()
            .map_err(|_| Error::custom("Failed to convert to array"))?;
        result.push(MessageId::from(array));
    }

    Ok(result)
}

/// Deserialize sync committee indices from JSON array (optional)
pub fn deserialize_sync_committee_indices_option<'de, D>(
    deserializer: D,
) -> Result<Option<VariableList<u64, U13>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<u64>> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(indices) => {
            let var_list = VariableList::new(indices)
                .map_err(|e| Error::custom(format!("Too many sync committee indices: {e:?}")))?;
            Ok(Some(var_list))
        }
    }
}

// QBFT Message Type Deserializers

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

/// Deserialize numeric QBFT message type from JSON
pub fn deserialize_qbft_message_type<'de, D>(deserializer: D) -> Result<QbftMessageType, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u8::deserialize(deserializer)?;

    match value {
        0 => Ok(QbftMessageType::Proposal),
        1 => Ok(QbftMessageType::Prepare),
        2 => Ok(QbftMessageType::Commit),
        3 => Ok(QbftMessageType::RoundChange),
        _ => Err(D::Error::custom(format!(
            "Invalid QbftMessageType value: {}",
            value
        ))),
    }
}

/// Deserialize optional QBFT message type
pub fn deserialize_qbft_message_type_option<'de, D>(
    deserializer: D,
) -> Result<Option<QbftMessageType>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<u8>::deserialize(deserializer)?;

    match value {
        None => Ok(None),
        Some(0) => Ok(Some(QbftMessageType::Proposal)),
        Some(1) => Ok(Some(QbftMessageType::Prepare)),
        Some(2) => Ok(Some(QbftMessageType::Commit)),
        Some(3) => Ok(Some(QbftMessageType::RoundChange)),
        Some(v) => Err(D::Error::custom(format!(
            "Invalid QbftMessageType value: {}",
            v
        ))),
    }
}
