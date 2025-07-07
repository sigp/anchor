use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType,
    utils::deserializers::arbitrary_object_parse::*,
};

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
        // Try deserializing as each type until one succeeds
        let (object_type, actual_size) = match ObjectDeserializer::try_all_types(&self.object) {
            Ok(result) => (result.object_type, result.encoded_size),
            Err(_) => return false,
        };

        // Validate size
        if actual_size != self.expected_encoded_length {
            return false;
        }

        // Additional validation for max size tests
        SszConstraintValidator::validate(&object_type, &self.object, self.is_max_size).is_ok()
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::MaxMsgSize)
    }
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
                        .map_err(|e| format!("Failed to decode signature {i}: {e}"))?;
                    Self::validate_constraint(
                        decoded.len(),
                        ssz_constraints::SIGNATURE_SIZE,
                        &format!("Signature {i} size"),
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
                .map_err(|e| format!("Failed to decode FullData: {e}"))?;
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
                .map_err(|e| format!("Failed to decode Data: {e}"))?;
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
        // RoundChangeJustification [][]byte `ssz-max:"13,51852"`  // Max 13 justifications, each
        // max 51852 bytes PrepareJustification     [][]byte `ssz-max:"13,3700"`   // Max 13
        // justifications, each max 3700 bytes

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
                        format!("Failed to decode RoundChangeJustification {i}: {e}")
                    })?;
                    Self::validate_constraint(
                        decoded.len(),
                        ssz_constraints::MAX_ROUND_CHANGE_JUSTIFICATION_SIZE,
                        &format!("RoundChangeJustification {i} size"),
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
                    let decoded = STANDARD
                        .decode(just_str)
                        .map_err(|e| format!("Failed to decode PrepareJustification {i}: {e}"))?;
                    Self::validate_constraint(
                        decoded.len(),
                        ssz_constraints::MAX_PREPARE_JUSTIFICATION_SIZE,
                        &format!("PrepareJustification {i} size"),
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
                    "{field_name} is different than ssz max size: {actual} != {max_size}"
                ));
            }
        } else if actual > max_size {
            return Err(format!(
                "{field_name} is bigger than ssz max size: {actual} > {max_size}"
            ));
        }
        Ok(())
    }
}
