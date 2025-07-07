use crate::utils::deserializers::validator_consensus_data_parse::*;
use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::Value;
use ssv_types::{
    consensus::{BeaconVote, QbftMessage, QbftMessageType, ValidatorConsensusData},
    message::{SSVMessage, SignedSSVMessage},
    partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
};
use ssz::Encode;
use types::{Checkpoint, Hash256, Signature, Slot, VariableList};

#[derive(Debug)]
enum ObjectType {
    BeaconVote,
    PartialSignatureMessage,
    PartialSignatureMessages,
    QbftMessage,
    SSVMessage,
    SignedSSVMessage,
    ValidatorConsensusData,
}

/// SSZ protocol size constraints matching the Go implementation
mod ssz_constraints {
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
struct ValidationResult {
    object_type: ObjectType,
    encoded_size: usize,
}

/// Polymorphic object deserializer that tries each type until one succeeds
struct ObjectDeserializer;

impl ObjectDeserializer {
    fn try_all_types(json: &Value) -> Result<ValidationResult, String> {
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
        let obj: SignedSSVMessage =
            serde_json::from_value(json.clone()).map_err(|e| format!("SignedSSVMessage: {}", e))?;
        Ok(obj.as_ssz_bytes().len())
    }

    fn try_ssv_message(json: &Value) -> Result<usize, String> {
        let obj: SSVMessage =
            serde_json::from_value(json.clone()).map_err(|e| format!("SSVMessage: {}", e))?;
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
        let obj = try_parse_validator_consensus_data(json)?;
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
        .map_err(|e| format!("Invalid BlockRoot: {}", e))?;

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
            .map_err(|e| format!("Invalid source epoch: {}", e))?,
        root: source_obj
            .get("root")
            .and_then(|v| v.as_str())
            .ok_or("Missing source root")?
            .parse::<Hash256>()
            .map_err(|e| format!("Invalid source root: {}", e))?,
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
            .map_err(|e| format!("Invalid target epoch: {}", e))?,
        root: target_obj
            .get("root")
            .and_then(|v| v.as_str())
            .ok_or("Missing target root")?
            .parse::<Hash256>()
            .map_err(|e| format!("Invalid target root: {}", e))?,
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
        .map_err(|e| format!("Failed to decode PartialSignature: {}", e))?;
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
        .map_err(|e| format!("Invalid ValidatorIndex: {}", e))?;

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
    let slot = Slot::new(
        slot_str
            .parse()
            .map_err(|e| format!("Invalid Slot: {}", e))?,
    );

    let messages_array = obj
        .get("Messages")
        .and_then(|v| v.as_array())
        .ok_or("Missing Messages array")?;

    let mut messages = Vec::new();
    for msg_json in messages_array {
        messages.push(deserialize_partial_signature_message(msg_json)?);
    }

    let messages = VariableList::new(messages)
        .map_err(|e| format!("Too many partial signature messages: {:?}", e))?;

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
        _ => return Err(format!("Invalid MsgType: {}", msg_type_num)),
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
        .map_err(|e| format!("Failed to decode Identifier: {}", e))?;
    let identifier = VariableList::new(identifier_bytes)
        .map_err(|e| format!("Identifier too large: {:?}", e))?;

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
                        format!("Failed to decode round change justification: {}", e)
                    })?;
                    let justification = VariableList::new(bytes)
                        .map_err(|e| format!("Round change justification too large: {:?}", e))?;
                    justifications.push(justification);
                }
            }
            VariableList::new(justifications)
                .map_err(|e| format!("Too many round change justifications: {:?}", e))?
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
                        .map_err(|e| format!("Failed to decode prepare justification: {}", e))?;
                    let justification = VariableList::new(bytes)
                        .map_err(|e| format!("Prepare justification too large: {:?}", e))?;
                    justifications.push(justification);
                }
            }
            VariableList::new(justifications)
                .map_err(|e| format!("Too many prepare justifications: {:?}", e))?
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

fn deserialize_ssv_message(json: &serde_json::Value) -> Result<SSVMessage, String> {
    // The JSON data format is correct - MsgID as byte array, Data as base64 string
    // Just use serde directly since MessageId has proper Deserialize implementation
    serde_json::from_value(json.clone())
        .map_err(|e| format!("Failed to deserialize SSVMessage: {}", e))
}

fn deserialize_signed_ssv_message(json: &serde_json::Value) -> Result<SignedSSVMessage, String> {
    serde_json::from_value(json.clone())
        .map_err(|e| format!("Failed to deserialize SignedSSVMessage: {}", e))
}

fn deserialize_validator_consensus_data(
    json: &serde_json::Value,
) -> Result<ValidatorConsensusData, String> {
    try_parse_validator_consensus_data(json)
}

/// SSZ constraint validator for protocol compliance
struct SszConstraintValidator;

impl SszConstraintValidator {
    fn validate(
        object_type: &ObjectType,
        json: &serde_json::Value,
        must_be_exact: bool,
    ) -> Result<(), String> {
        let obj = json.as_object().ok_or("Expected object")?;

        match object_type {
            ObjectType::SignedSSVMessage => Self::validate_signed_ssv_message(obj, must_be_exact),
            ObjectType::SSVMessage => Self::validate_ssv_message(obj, must_be_exact),
            ObjectType::PartialSignatureMessages => {
                Self::validate_partial_signature_messages(obj, must_be_exact)
            }
            ObjectType::QbftMessage => Self::validate_qbft_message(obj, must_be_exact),
            // Other types don't have notable SSZ constraints to validate
            _ => Ok(()),
        }
    }

    fn validate_signed_ssv_message(
        obj: &serde_json::Map<String, Value>,
        must_be_exact: bool,
    ) -> Result<(), String> {
        // SignedSSVMessage constraints from Go:
        // Signatures  [][]byte     `ssz-max:"13,256"`  // Max 13 signatures, each max 256 bytes
        // OperatorIDs []OperatorID `ssz-max:"13"`      // Max 13 operator IDs
        // FullData    []byte       `ssz-max:"8388836"` // Max ~8.4MB full data

        if let Some(signatures) = obj.get("Signatures").and_then(|v| v.as_array()) {
            Self::validate_constraint(
                signatures.len(),
                ssz_constraints::MAX_SIGNATURES,
                "Signatures count",
                must_be_exact,
            )?;

            // Each signature should be 256 bytes when base64 decoded
            for (i, sig) in signatures.iter().enumerate() {
                if let Some(sig_str) = sig.as_str() {
                    let decoded = STANDARD
                        .decode(sig_str)
                        .map_err(|e| format!("Failed to decode signature {}: {}", i, e))?;
                    Self::validate_constraint(
                        decoded.len(),
                        ssz_constraints::SIGNATURE_SIZE,
                        &format!("Signature {} size", i),
                        must_be_exact,
                    )?;
                }
            }
        }

        if let Some(operator_ids) = obj.get("OperatorIDs").and_then(|v| v.as_array()) {
            Self::validate_constraint(
                operator_ids.len(),
                ssz_constraints::MAX_OPERATOR_IDS,
                "OperatorIDs count",
                must_be_exact,
            )?;
        }

        if let Some(full_data_str) = obj.get("FullData").and_then(|v| v.as_str()) {
            let decoded = STANDARD
                .decode(full_data_str)
                .map_err(|e| format!("Failed to decode FullData: {}", e))?;
            Self::validate_constraint(
                decoded.len(),
                ssz_constraints::MAX_FULL_DATA_SIZE,
                "FullData size",
                must_be_exact,
            )?;
        }

        Ok(())
    }

    fn validate_ssv_message(
        obj: &serde_json::Map<String, Value>,
        must_be_exact: bool,
    ) -> Result<(), String> {
        // SSVMessage constraints from Go:
        // Data []byte `ssz-max:"722412"`  // Max ~722KB data

        if let Some(data_str) = obj.get("Data").and_then(|v| v.as_str()) {
            let decoded = STANDARD
                .decode(data_str)
                .map_err(|e| format!("Failed to decode Data: {}", e))?;
            Self::validate_constraint(
                decoded.len(),
                ssz_constraints::MAX_SSV_DATA_SIZE,
                "Data size",
                must_be_exact,
            )?;
        }

        Ok(())
    }

    fn validate_partial_signature_messages(
        obj: &serde_json::Map<String, Value>,
        must_be_exact: bool,
    ) -> Result<(), String> {
        // PartialSignatureMessages constraints:
        // Messages []PartialSignatureMessage `ssz-max:"1512"`  // Max 1512 messages

        if let Some(messages) = obj.get("Messages").and_then(|v| v.as_array()) {
            Self::validate_constraint(
                messages.len(),
                ssz_constraints::MAX_PARTIAL_SIG_MESSAGES,
                "Messages count",
                must_be_exact,
            )?;
        }

        Ok(())
    }

    fn validate_qbft_message(
        obj: &serde_json::Map<String, Value>,
        must_be_exact: bool,
    ) -> Result<(), String> {
        // QbftMessage constraints from Go:
        // RoundChangeJustification [][]byte `ssz-max:"13,51852"`  // Max 13 justifications, each max 51852 bytes
        // PrepareJustification     [][]byte `ssz-max:"13,3700"`   // Max 13 justifications, each max 3700 bytes

        if let Some(rc_just) = obj
            .get("RoundChangeJustification")
            .and_then(|v| v.as_array())
        {
            Self::validate_constraint(
                rc_just.len(),
                ssz_constraints::MAX_JUSTIFICATIONS,
                "RoundChangeJustification count",
                must_be_exact,
            )?;

            for (i, just) in rc_just.iter().enumerate() {
                if let Some(just_str) = just.as_str() {
                    let decoded = STANDARD.decode(just_str).map_err(|e| {
                        format!("Failed to decode RoundChangeJustification {}: {}", i, e)
                    })?;
                    Self::validate_constraint(
                        decoded.len(),
                        ssz_constraints::MAX_ROUND_CHANGE_JUSTIFICATION_SIZE,
                        &format!("RoundChangeJustification {} size", i),
                        must_be_exact,
                    )?;
                }
            }
        }

        if let Some(prep_just) = obj.get("PrepareJustification").and_then(|v| v.as_array()) {
            Self::validate_constraint(
                prep_just.len(),
                ssz_constraints::MAX_JUSTIFICATIONS,
                "PrepareJustification count",
                must_be_exact,
            )?;

            for (i, just) in prep_just.iter().enumerate() {
                if let Some(just_str) = just.as_str() {
                    let decoded = STANDARD.decode(just_str).map_err(|e| {
                        format!("Failed to decode PrepareJustification {}: {}", i, e)
                    })?;
                    Self::validate_constraint(
                        decoded.len(),
                        ssz_constraints::MAX_PREPARE_JUSTIFICATION_SIZE,
                        &format!("PrepareJustification {} size", i),
                        must_be_exact,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn validate_constraint(
        actual: usize,
        max_size: usize,
        field_name: &str,
        must_be_exact: bool,
    ) -> Result<(), String> {
        if must_be_exact {
            if actual != max_size {
                return Err(format!(
                    "{} is different than ssz max size: {} != {}",
                    field_name, actual, max_size
                ));
            }
        } else {
            if actual > max_size {
                return Err(format!(
                    "{} is bigger than ssz max size: {} > {}",
                    field_name, actual, max_size
                ));
            }
        }
        Ok(())
    }
}

// we require a new parsing structure
// Structure size validation test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaxMsgSizeTest {
    #[serde(rename = "Name")]
    pub name: String,
    // Use generic Json value since object differs for test
    #[serde(rename = "Object")]
    pub object: serde_json::Value,
    #[serde(rename = "ExpectedEncodedLength")]
    pub expected_encoded_length: usize,
    #[serde(rename = "IsMaxSize")]
    pub is_max_size: bool,
}

impl SpecTest for MaxMsgSizeTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        // Try deserializing as each type until one succeeds (Go approach)
        let validation_result = match ObjectDeserializer::try_all_types(&self.object) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("Failed to deserialize test '{}': {}", self.name, e);
                return false;
            }
        };
        let (object_type, actual_size) = (
            validation_result.object_type,
            validation_result.encoded_size,
        );

        // Validate size
        if actual_size != self.expected_encoded_length {
            eprintln!(
                "Size mismatch for test '{}' (detected as {:?}): expected {}, got {}",
                self.name, object_type, self.expected_encoded_length, actual_size
            );
            return false;
        }

        // Additional validation for max size tests
        if self.is_max_size {
            // For max size tests, validate that the object uses maximum SSZ constraints
            match SszConstraintValidator::validate(&object_type, &self.object, true) {
                Ok(()) => {} // SSZ constraints satisfied
                Err(e) => {
                    eprintln!(
                        "SSZ constraint validation failed for test '{}': {}",
                        self.name, e
                    );
                    return false;
                }
            }
        } else {
            // For expected size tests, validate that the object is within SSZ constraints
            match SszConstraintValidator::validate(&object_type, &self.object, false) {
                Ok(()) => {} // SSZ constraints satisfied
                Err(e) => {
                    eprintln!(
                        "SSZ constraint validation failed for test '{}': {}",
                        self.name, e
                    );
                    return false;
                }
            }
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::MaxMsgSize)
    }
}
