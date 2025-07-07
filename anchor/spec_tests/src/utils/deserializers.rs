use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, de::Error};
use ssv_types::{
    ValidatorIndex,
    consensus::{
        BEACON_ROLE_AGGREGATOR, BEACON_ROLE_ATTESTER, BEACON_ROLE_PROPOSER,
        BEACON_ROLE_SYNC_COMMITTEE, BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION,
        BEACON_ROLE_VALIDATOR_REGISTRATION, BEACON_ROLE_VOLUNTARY_EXIT, BeaconRole, DataVersion,
        ValidatorConsensusData, ValidatorDuty,
    },
    message::ValidatorConsensusDataLen,
};
use types::{CommitteeIndex, ForkName, Hash256, PublicKeyBytes, Slot, VariableList, typenum::U13};

/// General type parsers
pub mod type_parse {
    use super::*;
    // Convert base64 string to bytes
    pub fn deserialize_base64_to_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let base64_string = String::deserialize(deserializer)?;
        STANDARD
            .decode(&base64_string)
            .map_err(|e| Error::custom(format!("Failed to decode base64 string: {e}")))
    }

    // Convert byte array to Hash256 for expected roots
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

    // Convert optional base64 into bytes
    pub fn deserialize_base64_option_to_bytes<'de, D>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(s) => STANDARD.decode(&s).map(Some).map_err(D::Error::custom),
            None => Ok(None),
        }
    }

    // Deserialize optional vector of Hash256 from byte arrays
    pub fn deserialize_optional_hash256_vec<'de, D>(
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

    // Deserialize optional vector of base64 encoded byte arrays
    pub fn deserialize_optional_base64_vec<'de, D>(
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
                    let bytes = STANDARD.decode(&s).map_err(|e| {
                        Error::custom(format!("Failed to decode base64 string: {e}"))
                    })?;
                    result.push(bytes);
                }
                Ok(Some(result))
            }
        }
    }
}

/// Logic for parsing json validator consensus data
pub mod validator_consensus_data_parse {
    use super::*;

    /// Parse JSON Value directly to ValidatorConsensusData with comprehensive error handling
    pub fn try_parse_validator_consensus_data(
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
            format!("0x{hex_str}")
        };

        hex_with_prefix
            .parse()
            .map_err(|e| format!("Invalid public key: {e}"))
    }

    fn parse_slot(value: Option<&serde_json::Value>) -> Result<Slot, String> {
        let slot_str = value
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid Slot")?;
        let slot_num: u64 = slot_str.parse().map_err(|e| format!("Invalid slot: {e}"))?;
        Ok(Slot::new(slot_num))
    }

    fn parse_validator_index(value: Option<&serde_json::Value>) -> Result<ValidatorIndex, String> {
        let index_str = value
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid ValidatorIndex")?;
        let index_num: usize = index_str
            .parse()
            .map_err(|e| format!("Invalid validator index: {e}"))?;
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
            .ok_or_else(|| format!("Missing or invalid {field_name}"))
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
                    .map_err(|e| format!("Too many sync committee indices: {e:?}"))
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
            _ => Err(format!("Invalid version: {version_str}")),
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
            .map_err(|e| format!("Failed to decode base64 DataSSZ: {e}"))?;
        VariableList::new(bytes).map_err(|e| format!("DataSSZ too large: {e:?}"))
    }
}

/// Module defining logic for dynamically parsing when a test category uses multiple json
/// structures
pub mod arbitrary_object_parse {
    use serde_json::Value;
    use ssv_types::{
        consensus::{BeaconVote, QbftMessage, QbftMessageType},
        message::{SSVMessage, SignedSSVMessage},
        partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
    };
    use ssz::Encode;
    use types::{Checkpoint, Hash256, Signature, Slot, VariableList};

    use super::*;

    #[derive(Debug)]
    pub enum ObjectType {
        BeaconVote,
        PartialSignatureMessage,
        PartialSignatureMessages,
        QbftMessage,
        SSVMessage,
        SignedSSVMessage,
        ValidatorConsensusData,
    }

    /// SSZ protocol size constraints matching the Go implementation
    pub mod ssz_constraints {
        pub const MAX_SIGNATURES: usize = 13;
        pub const SIGNATURE_SIZE: usize = 256;
        pub const MAX_OPERATOR_IDS: usize = 13;
        pub const MAX_FULL_DATA_SIZE: usize = 8388836;
        pub const MAX_SSV_DATA_SIZE: usize = 722412;
        pub const MAX_PARTIAL_SIG_MESSAGES: usize = 1512;
        pub const MAX_JUSTIFICATIONS: usize = 13;
        pub const MAX_ROUND_CHANGE_JUSTIFICATION_SIZE: usize = 51852;
        pub const MAX_PREPARE_JUSTIFICATION_SIZE: usize = 3700;
    }

    /// Result of attempting to deserialize and validate an object
    pub struct ValidationResult {
        pub object_type: ObjectType,
        pub encoded_size: usize,
    }

    /// Polymorphic object deserializer that tries each type until one succeeds
    pub struct ObjectDeserializer;
    impl ObjectDeserializer {
        pub fn try_all_types(json: &Value) -> Result<ValidationResult, String> {
            // Try each type in order matching Go implementation approach
            if let Ok(size) = Self::try_signed_ssv_message(json) {
                return Ok(ValidationResult {
                    object_type: ObjectType::SignedSSVMessage,
                    encoded_size: size,
                });
            }

            if let Ok(size) = Self::try_ssv_message(json) {
                return Ok(ValidationResult {
                    object_type: ObjectType::SSVMessage,
                    encoded_size: size,
                });
            }

            if let Ok(size) = Self::try_qbft_message(json) {
                return Ok(ValidationResult {
                    object_type: ObjectType::QbftMessage,
                    encoded_size: size,
                });
            }

            if let Ok(size) = Self::try_partial_signature_messages(json) {
                return Ok(ValidationResult {
                    object_type: ObjectType::PartialSignatureMessages,
                    encoded_size: size,
                });
            }

            if let Ok(size) = Self::try_partial_signature_message(json) {
                return Ok(ValidationResult {
                    object_type: ObjectType::PartialSignatureMessage,
                    encoded_size: size,
                });
            }

            if let Ok(size) = Self::try_validator_consensus_data(json) {
                return Ok(ValidationResult {
                    object_type: ObjectType::ValidatorConsensusData,
                    encoded_size: size,
                });
            }

            if let Ok(size) = Self::try_beacon_vote(json) {
                return Ok(ValidationResult {
                    object_type: ObjectType::BeaconVote,
                    encoded_size: size,
                });
            }

            Err("Could not deserialize as any known type".to_string())
        }

        fn try_signed_ssv_message(json: &Value) -> Result<usize, String> {
            let obj: SignedSSVMessage = serde_json::from_value(json.clone())
                .map_err(|e| format!("SignedSSVMessage: {e}"))?;
            Ok(obj.as_ssz_bytes().len())
        }

        fn try_ssv_message(json: &Value) -> Result<usize, String> {
            let obj: SSVMessage =
                serde_json::from_value(json.clone()).map_err(|e| format!("SSVMessage: {e}"))?;
            Ok(obj.as_ssz_bytes().len())
        }

        fn try_qbft_message(json: &Value) -> Result<usize, String> {
            let obj = deserialize_qbft_message(json)?;
            Ok(obj.as_ssz_bytes().len())
        }

        fn try_partial_signature_messages(json: &Value) -> Result<usize, String> {
            let obj = deserialize_partial_signature_messages(json)?;
            Ok(obj.as_ssz_bytes().len())
        }

        fn try_partial_signature_message(json: &Value) -> Result<usize, String> {
            let obj = deserialize_partial_signature_message(json)?;
            Ok(obj.as_ssz_bytes().len())
        }

        fn try_validator_consensus_data(json: &Value) -> Result<usize, String> {
            let obj = validator_consensus_data_parse::try_parse_validator_consensus_data(json)?;
            Ok(obj.as_ssz_bytes().len())
        }

        fn try_beacon_vote(json: &Value) -> Result<usize, String> {
            let obj = deserialize_beacon_vote(json)?;
            Ok(obj.as_ssz_bytes().len())
        }
    }

    fn deserialize_beacon_vote(json: &serde_json::Value) -> Result<BeaconVote, String> {
        let obj = json.as_object().ok_or("Expected object")?;

        let block_root = obj
            .get("BlockRoot")
            .and_then(|v| v.as_str())
            .ok_or("Missing BlockRoot")?
            .parse::<Hash256>()
            .map_err(|e| format!("Invalid BlockRoot: {e}"))?;

        let source_obj = obj
            .get("Source")
            .and_then(|v| v.as_object())
            .ok_or("Missing Source")?;
        let source = Checkpoint {
            epoch: source_obj
                .get("epoch")
                .and_then(|v| v.as_str())
                .ok_or("Missing source epoch")?
                .parse()
                .map_err(|e| format!("Invalid source epoch: {e}"))?,
            root: source_obj
                .get("root")
                .and_then(|v| v.as_str())
                .ok_or("Missing source root")?
                .parse::<Hash256>()
                .map_err(|e| format!("Invalid source root: {e}"))?,
        };

        let target_obj = obj
            .get("Target")
            .and_then(|v| v.as_object())
            .ok_or("Missing Target")?;
        let target = Checkpoint {
            epoch: target_obj
                .get("epoch")
                .and_then(|v| v.as_str())
                .ok_or("Missing target epoch")?
                .parse()
                .map_err(|e| format!("Invalid target epoch: {e}"))?,
            root: target_obj
                .get("root")
                .and_then(|v| v.as_str())
                .ok_or("Missing target root")?
                .parse::<Hash256>()
                .map_err(|e| format!("Invalid target root: {e}"))?,
        };

        Ok(BeaconVote {
            block_root,
            source,
            target,
        })
    }

    fn deserialize_partial_signature_message(
        json: &serde_json::Value,
    ) -> Result<PartialSignatureMessage, String> {
        let obj = json.as_object().ok_or("Expected object")?;

        let partial_signature_str = obj
            .get("PartialSignature")
            .and_then(|v| v.as_str())
            .ok_or("Missing PartialSignature")?;
        let partial_signature_bytes = STANDARD
            .decode(partial_signature_str)
            .map_err(|e| format!("Failed to decode PartialSignature: {e}"))?;
        if partial_signature_bytes.len() != 96 {
            return Err(format!(
                "PartialSignature must be 96 bytes, got {}",
                partial_signature_bytes.len()
            ));
        }

        // For size testing, we don't need valid BLS signatures, just well-formed data
        // If the signature fails to deserialize, create an empty one with the same size
        let partial_signature = match Signature::deserialize(&partial_signature_bytes) {
            Ok(sig) => sig,
            Err(_) => {
                // Create empty signature for size testing purposes
                // This maintains the correct SSZ size without requiring valid cryptography
                Signature::empty()
            }
        };

        let signing_root_array = obj
            .get("SigningRoot")
            .and_then(|v| v.as_array())
            .ok_or("Missing SigningRoot array")?;
        if signing_root_array.len() != 32 {
            return Err(format!(
                "SigningRoot must be 32 bytes, got {}",
                signing_root_array.len()
            ));
        }
        let mut signing_root_bytes = [0u8; 32];
        for (i, val) in signing_root_array.iter().enumerate() {
            signing_root_bytes[i] = val.as_u64().ok_or("Invalid SigningRoot byte")? as u8;
        }
        let signing_root = Hash256::from_slice(&signing_root_bytes);

        let signer = obj
            .get("Signer")
            .and_then(|v| v.as_u64())
            .ok_or("Missing Signer")?;

        let validator_index_str = obj
            .get("ValidatorIndex")
            .and_then(|v| v.as_str())
            .ok_or("Missing ValidatorIndex")?;
        let validator_index: usize = validator_index_str
            .parse()
            .map_err(|e| format!("Invalid ValidatorIndex: {e}"))?;

        Ok(PartialSignatureMessage {
            partial_signature,
            signing_root,
            signer: ssv_types::OperatorId(signer),
            validator_index: ssv_types::ValidatorIndex(validator_index),
        })
    }

    fn deserialize_partial_signature_messages(
        json: &serde_json::Value,
    ) -> Result<PartialSignatureMessages, String> {
        let obj = json.as_object().ok_or("Expected object")?;

        let kind_num = obj
            .get("Type")
            .and_then(|v| v.as_u64())
            .ok_or("Missing Type")?;
        let kind = PartialSignatureKind::from(kind_num);

        let slot_str = obj
            .get("Slot")
            .and_then(|v| v.as_str())
            .ok_or("Missing Slot")?;
        let slot = Slot::new(slot_str.parse().map_err(|e| format!("Invalid Slot: {e}"))?);

        let messages_array = obj
            .get("Messages")
            .and_then(|v| v.as_array())
            .ok_or("Missing Messages array")?;

        let mut messages = Vec::new();
        for msg_json in messages_array {
            messages.push(deserialize_partial_signature_message(msg_json)?);
        }

        let messages = VariableList::new(messages)
            .map_err(|e| format!("Too many partial signature messages: {e:?}"))?;

        Ok(PartialSignatureMessages {
            kind,
            slot,
            messages,
        })
    }

    fn deserialize_qbft_message(json: &serde_json::Value) -> Result<QbftMessage, String> {
        let obj = json.as_object().ok_or("Expected object")?;

        let msg_type_num = obj
            .get("MsgType")
            .and_then(|v| v.as_u64())
            .ok_or("Missing MsgType")?;
        let qbft_message_type = match msg_type_num {
            0 => QbftMessageType::Proposal,
            1 => QbftMessageType::Prepare,
            2 => QbftMessageType::Commit,
            3 => QbftMessageType::RoundChange,
            _ => return Err(format!("Invalid MsgType: {msg_type_num}")),
        };

        let height = obj
            .get("Height")
            .and_then(|v| v.as_u64())
            .ok_or("Missing Height")?;

        let round = obj
            .get("Round")
            .and_then(|v| v.as_u64())
            .ok_or("Missing Round")?;

        let identifier_str = obj
            .get("Identifier")
            .and_then(|v| v.as_str())
            .ok_or("Missing Identifier")?;
        let identifier_bytes = STANDARD
            .decode(identifier_str)
            .map_err(|e| format!("Failed to decode Identifier: {e}"))?;
        let identifier = VariableList::new(identifier_bytes)
            .map_err(|e| format!("Identifier too large: {e:?}"))?;

        let root_array = obj
            .get("Root")
            .and_then(|v| v.as_array())
            .ok_or("Missing Root array")?;
        if root_array.len() != 32 {
            return Err(format!("Root must be 32 bytes, got {}", root_array.len()));
        }
        let mut root_bytes = [0u8; 32];
        for (i, val) in root_array.iter().enumerate() {
            root_bytes[i] = val.as_u64().ok_or("Invalid Root byte")? as u8;
        }
        let root = Hash256::from_slice(&root_bytes);

        let data_round = obj
            .get("DataRound")
            .and_then(|v| v.as_u64())
            .ok_or("Missing DataRound")?;

        // Handle justifications - they can be null, empty arrays, or arrays of base64 strings
        let round_change_justification = match obj.get("RoundChangeJustification") {
            Some(serde_json::Value::Array(arr)) => {
                let mut justifications = Vec::new();
                for item in arr {
                    if let Some(b64_str) = item.as_str() {
                        let bytes = STANDARD.decode(b64_str).map_err(|e| {
                            format!("Failed to decode round change justification: {e}")
                        })?;
                        let justification = VariableList::new(bytes)
                            .map_err(|e| format!("Round change justification too large: {e:?}"))?;
                        justifications.push(justification);
                    }
                }
                VariableList::new(justifications)
                    .map_err(|e| format!("Too many round change justifications: {e:?}"))?
            }
            _ => VariableList::empty(),
        };

        let prepare_justification = match obj.get("PrepareJustification") {
            Some(serde_json::Value::Array(arr)) => {
                let mut justifications = Vec::new();
                for item in arr {
                    if let Some(b64_str) = item.as_str() {
                        let bytes = STANDARD
                            .decode(b64_str)
                            .map_err(|e| format!("Failed to decode prepare justification: {e}"))?;
                        let justification = VariableList::new(bytes)
                            .map_err(|e| format!("Prepare justification too large: {e:?}"))?;
                        justifications.push(justification);
                    }
                }
                VariableList::new(justifications)
                    .map_err(|e| format!("Too many prepare justifications: {e:?}"))?
            }
            _ => VariableList::empty(),
        };

        Ok(QbftMessage {
            qbft_message_type,
            height,
            round,
            identifier,
            root,
            data_round,
            round_change_justification,
            prepare_justification,
        })
    }
}
