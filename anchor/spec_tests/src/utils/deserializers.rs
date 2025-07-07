use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, de::Error};
use ssv_types::consensus::{
    BEACON_ROLE_AGGREGATOR, BEACON_ROLE_ATTESTER, BEACON_ROLE_PROPOSER, BEACON_ROLE_SYNC_COMMITTEE,
    BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION, BEACON_ROLE_VALIDATOR_REGISTRATION,
    BEACON_ROLE_VOLUNTARY_EXIT, BeaconRole, DataVersion, ValidatorConsensusData, ValidatorDuty,
};
use ssv_types::{ValidatorIndex, message::ValidatorConsensusDataLen};
use ssz::Decode;
use tree_hash::TreeHash;
use types::typenum::U13;
use types::{
    BeaconBlock, BlindedBeaconBlock, CommitteeIndex, ForkName, Hash256, MainnetEthSpec,
    PublicKeyBytes, Slot, VariableList,
};

/// Parse JSON Value directly to ValidatorConsensusData with comprehensive error handling
pub(crate) fn try_parse_validator_consensus_data(
    value: &serde_json::Value,
) -> Result<ValidatorConsensusData, String> {
    let obj = value.as_object().ok_or("Expected object")?;

    // Parse Duty object
    let duty_obj = obj
        .get("Duty")
        .and_then(|v| v.as_object())
        .ok_or("Missing or invalid Duty")?;
    let duty = ValidatorDuty {
        r#type: parse_beacon_role(duty_obj.get("Type"))?,
        pub_key: parse_public_key(duty_obj.get("PubKey"))?,
        slot: parse_slot(duty_obj.get("Slot"))?,
        validator_index: parse_validator_index(duty_obj.get("ValidatorIndex"))?,
        committee_index: parse_committee_index(duty_obj.get("CommitteeIndex"))?,
        committee_length: parse_u64(duty_obj.get("CommitteeLength"), "CommitteeLength")?,
        committees_at_slot: parse_u64(duty_obj.get("CommitteesAtSlot"), "CommitteesAtSlot")?,
        validator_committee_index: parse_u64(
            duty_obj.get("ValidatorCommitteeIndex"),
            "ValidatorCommitteeIndex",
        )?,
        validator_sync_committee_indices: parse_sync_committee_indices(
            duty_obj.get("ValidatorSyncCommitteeIndices"),
        )?,
    };

    // Parse Version
    let version = parse_data_version(obj.get("Version"))?;

    // Parse DataSSZ
    let data_ssz = parse_data_ssz(obj.get("DataSSZ"))?;

    Ok(ValidatorConsensusData {
        duty,
        version,
        data_ssz,
    })
}

// Helper parsing functions (all private)
fn parse_beacon_role(value: Option<&serde_json::Value>) -> Result<BeaconRole, String> {
    let num = value
        .and_then(|v| v.as_u64())
        .ok_or("Missing or invalid Type")?;
    match num {
        0 => Ok(BEACON_ROLE_ATTESTER),
        1 => Ok(BEACON_ROLE_AGGREGATOR),
        2 => Ok(BEACON_ROLE_PROPOSER),
        3 => Ok(BEACON_ROLE_SYNC_COMMITTEE),
        4 => Ok(BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION),
        5 => Ok(BEACON_ROLE_VALIDATOR_REGISTRATION),
        6 => Ok(BEACON_ROLE_VOLUNTARY_EXIT),
        _ => Err("unknown duty role".to_string()),
    }
}

fn parse_public_key(value: Option<&serde_json::Value>) -> Result<PublicKeyBytes, String> {
    let hex_str = value
        .and_then(|v| v.as_str())
        .ok_or("Missing or invalid PubKey")?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);

    // Ensure the hex string has the 0x prefix for parsing
    let hex_with_prefix = if hex_str.starts_with("0x") {
        hex_str.to_string()
    } else {
        format!("0x{}", hex_str)
    };

    hex_with_prefix
        .parse()
        .map_err(|e| format!("Invalid public key: {}", e))
}

fn parse_slot(value: Option<&serde_json::Value>) -> Result<Slot, String> {
    let slot_str = value
        .and_then(|v| v.as_str())
        .ok_or("Missing or invalid Slot")?;
    let slot_num: u64 = slot_str
        .parse()
        .map_err(|e| format!("Invalid slot: {}", e))?;
    Ok(Slot::new(slot_num))
}

fn parse_validator_index(value: Option<&serde_json::Value>) -> Result<ValidatorIndex, String> {
    let index_str = value
        .and_then(|v| v.as_str())
        .ok_or("Missing or invalid ValidatorIndex")?;
    let index_num: usize = index_str
        .parse()
        .map_err(|e| format!("Invalid validator index: {}", e))?;
    Ok(ValidatorIndex(index_num))
}

fn parse_committee_index(value: Option<&serde_json::Value>) -> Result<CommitteeIndex, String> {
    let num = value
        .and_then(|v| v.as_u64())
        .ok_or("Missing or invalid CommitteeIndex")?;
    Ok(CommitteeIndex::from(num))
}

fn parse_u64(value: Option<&serde_json::Value>, field_name: &str) -> Result<u64, String> {
    value
        .and_then(|v| v.as_u64())
        .ok_or_else(|| format!("Missing or invalid {}", field_name))
}

fn parse_sync_committee_indices(
    value: Option<&serde_json::Value>,
) -> Result<VariableList<u64, U13>, String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(VariableList::empty()),
        Some(serde_json::Value::Array(arr)) => {
            let indices: Result<Vec<u64>, String> = arr
                .iter()
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| "Invalid sync committee index".to_string())
                })
                .collect();
            let indices = indices?;
            VariableList::new(indices)
                .map_err(|e| format!("Too many sync committee indices: {:?}", e))
        }
        _ => Err("Invalid ValidatorSyncCommitteeIndices format".to_string()),
    }
}

fn parse_data_version(value: Option<&serde_json::Value>) -> Result<DataVersion, String> {
    let version_str = value
        .and_then(|v| v.as_str())
        .ok_or("Missing or invalid Version")?;
    match version_str {
        "phase0" => Ok(DataVersion::from(ForkName::Base)),
        "altair" => Ok(DataVersion::from(ForkName::Altair)),
        "bellatrix" => Ok(DataVersion::from(ForkName::Bellatrix)),
        "capella" => Ok(DataVersion::from(ForkName::Capella)),
        "deneb" => Ok(DataVersion::from(ForkName::Deneb)),
        "electra" => Ok(DataVersion::from(ForkName::Electra)),
        "fulu" => Ok(DataVersion::from(ForkName::Fulu)),
        _ => Err(format!("Invalid version: {}", version_str)),
    }
}

fn parse_data_ssz(
    value: Option<&serde_json::Value>,
) -> Result<VariableList<u8, ValidatorConsensusDataLen>, String> {
    let base64_str = value
        .and_then(|v| v.as_str())
        .ok_or("Missing or invalid DataSSZ")?;
    let bytes = STANDARD
        .decode(base64_str)
        .map_err(|e| format!("Failed to decode base64 DataSSZ: {}", e))?;
    VariableList::new(bytes).map_err(|e| format!("DataSSZ too large: {:?}", e))
}

// Convert base64 string to bytes for data fields
pub(crate) fn deserialize_base64_to_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let base64_string = String::deserialize(deserializer)?;
    STANDARD
        .decode(&base64_string)
        .map_err(|e| Error::custom(format!("Failed to decode base64 string: {}", e)))
}

// Convert byte array to Hash256 for expected roots
pub(crate) fn deserialize_bytes_to_hash256<'de, D>(deserializer: D) -> Result<Hash256, D::Error>
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

// Convert string to Slot (u64)
pub(crate) fn deserialize_string_to_slot<'de, D>(deserializer: D) -> Result<Slot, D::Error>
where
    D: Deserializer<'de>,
{
    let slot_str = String::deserialize(deserializer)?;
    slot_str
        .parse::<u64>()
        .map(Slot::new)
        .map_err(|e| Error::custom(format!("Failed to parse slot: {}", e)))
}

// Convert integer to OperatorId
pub(crate) fn deserialize_u64_to_operator_id<'de, D>(
    deserializer: D,
) -> Result<ssv_types::OperatorId, D::Error>
where
    D: Deserializer<'de>,
{
    let id = u64::deserialize(deserializer)?;
    Ok(ssv_types::OperatorId(id))
}

// Convert string to ValidatorIndex
pub(crate) fn deserialize_string_to_validator_index<'de, D>(
    deserializer: D,
) -> Result<ValidatorIndex, D::Error>
where
    D: Deserializer<'de>,
{
    let index_str = String::deserialize(deserializer)?;
    index_str
        .parse::<usize>()
        .map(ValidatorIndex)
        .map_err(|e| Error::custom(format!("Failed to parse validator index: {}", e)))
}

// Deserialize optional vector of base64 encoded byte arrays
pub(crate) fn deserialize_optional_base64_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Vec<u8>>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<String>> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(strings) => {
            let mut result = Vec::new();
            for s in strings {
                let bytes = STANDARD
                    .decode(&s)
                    .map_err(|e| Error::custom(format!("Failed to decode base64 string: {}", e)))?;
                result.push(bytes);
            }
            Ok(Some(result))
        }
    }
}

// Deserialize optional vector of Hash256 from byte arrays
pub(crate) fn deserialize_optional_hash256_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Hash256>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<Vec<u8>>> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(byte_arrays) => {
            let mut result = Vec::new();
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

/// Decode ValidatorConsensusData from raw SSZ bytes with proper error handling
pub(crate) fn decode_consensus_data_from_ssz(
    data: &[u8],
) -> Result<ValidatorConsensusData, String> {
    ValidatorConsensusData::from_ssz_bytes(data)
        .map_err(|e| format!("could not unmarshal ssz: {:?}", e))
}

/// Extract and validate block data from consensus data, returning the block root
pub(crate) fn extract_and_validate_block_data(
    consensus_data: &ValidatorConsensusData,
    is_blinded: bool,
) -> Result<Hash256, String> {
    let block_data = &consensus_data.data_ssz;

    // Log consensus data version for debugging
    eprintln!(
        "Consensus data version: {:?}, is_blinded: {}, block_data_len: {}",
        consensus_data.version,
        is_blinded,
        block_data.len()
    );

    // Use the version from consensus data instead of trying all forks
    let fork_name: ForkName = consensus_data.version.clone().into();

    if is_blinded {
        try_deserialize_blinded_block_for_fork(block_data, fork_name)
    } else {
        try_deserialize_regular_block_for_fork(block_data, fork_name)
    }
}

/// Try to deserialize blinded block data with a specific fork version and return tree hash root
pub(crate) fn try_deserialize_blinded_block_for_fork(
    block_data: &[u8],
    fork: ForkName,
) -> Result<Hash256, String> {
    match BlindedBeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(block_data, fork) {
        Ok(block) => Ok(block.tree_hash_root()),
        Err(e) => Err(format!(
            "Failed to deserialize blinded block for fork {:?}: {:?}",
            fork, e
        )),
    }
}

/// Try to deserialize regular block data with a specific fork version and return tree hash root
pub(crate) fn try_deserialize_regular_block_for_fork(
    block_data: &[u8],
    fork: ForkName,
) -> Result<Hash256, String> {
    match BeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(block_data, fork) {
        Ok(block) => Ok(block.tree_hash_root()),
        Err(e) => Err(format!(
            "Failed to deserialize regular block for fork {:?}: {:?}",
            fork, e
        )),
    }
}

/// Try to deserialize blinded block data with different fork versions and return tree hash root
pub(crate) fn try_deserialize_blinded_block_for_root(block_data: &[u8]) -> Result<Hash256, String> {
    let forks = [
        ForkName::Base,
        ForkName::Altair,
        ForkName::Bellatrix,
        ForkName::Capella,
        ForkName::Deneb,
        ForkName::Electra,
        ForkName::Fulu,
    ];

    let mut errors = Vec::new();
    for fork in forks {
        match BlindedBeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(block_data, fork) {
            Ok(block) => return Ok(block.tree_hash_root()),
            Err(e) => errors.push(format!("{:?}: {:?}", fork, e)),
        }
    }

    Err(format!(
        "unknown block version unknown. Tried forks: {}",
        errors.join(", ")
    ))
}

/// Try to deserialize regular block data with different fork versions and return tree hash root
pub(crate) fn try_deserialize_regular_block_for_root(block_data: &[u8]) -> Result<Hash256, String> {
    let forks = [
        ForkName::Base,
        ForkName::Altair,
        ForkName::Bellatrix,
        ForkName::Capella,
        ForkName::Deneb,
        ForkName::Electra,
        ForkName::Fulu,
    ];

    let mut errors = Vec::new();
    for fork in forks {
        match BeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(block_data, fork) {
            Ok(block) => return Ok(block.tree_hash_root()),
            Err(e) => errors.push(format!("{:?}: {:?}", fork, e)),
        }
    }

    Err(format!(
        "unknown block version unknown. Tried forks: {}",
        errors.join(", ")
    ))
}

/// Check if an error message matches expected error patterns
pub(crate) fn matches_expected_error(expected: &str, actual: &str) -> bool {
    actual.contains(expected)
        || expected.contains(actual)
        || (expected.contains("could not unmarshal ssz") && actual.contains("ssz"))
        || (expected.contains("unknown block version") && actual.contains("unknown"))
}
