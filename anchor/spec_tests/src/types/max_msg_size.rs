use crate::types::types_deserializers::*;
use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;
use serde_json;
use ssz::Encode;

// we require a new parsing structure

// Structure size validation test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructureSizeTest {
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

impl SpecTest for StructureSizeTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        println!("{:?}", self.name);
        // Parse the test name to determine what type of object we're testing
        let object_type = extract_object_type(&self.name);

        // Encode the object based on its type and get its length
        let encoded_length = match encode_object_by_type(&object_type, &self.object) {
            Ok(length) => length,
            Err(e) => {
                println!("{:?}", e);
                return false;
            }
        };

        // Check that the encoded length matches the expected length
        if encoded_length != self.expected_encoded_length {
            println!(
                "Length mismatch for {}: expected {}, got {}",
                object_type, self.expected_encoded_length, encoded_length
            );
            return false;
        }

        /*
                // For max size tests, validate that the size is within reasonable bounds
                if self.is_max_size {
                    let is_valid_max_size = validate_max_size(&object_type, encoded_length);
                    if !is_valid_max_size {
                        println!(
                            "Max size {} for {} exceeds reasonable bounds",
                            encoded_length, object_type
                        );
                        return false;
                    } else {
                        println!(
                            "✅ Max size {} for {} is within bounds",
                            encoded_length, object_type
                        );
                    }
                }
        */
        println!("Passed");
        println!("");

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::MaxMsgSize)
    }
}

/// Extract the object type from the test name
fn extract_object_type(name: &str) -> &str {
    if name.contains("BeaconVote") {
        "BeaconVote"
    } else if name.contains("PartialSignatureMessage") && !name.contains("PartialSignatureMessages")
    {
        "PartialSignatureMessage"
    } else if name.contains("qbftMessage") {
        "qbftMessage"
    } else if name.contains("SignedSSVMessage") {
        "SignedSSVMessage"
    } else if name.contains("SSVMessage") {
        "SSVMessage"
    } else if name.contains("PartialSignatureMessages") {
        "PartialSignatureMessages"
    } else if name.contains("ValidatorConsensusData") {
        "ValidatorConsensusData"
    } else {
        "Unknown"
    }
}

/// Encode an object based on its type and return the encoded length
fn encode_object_by_type(object_type: &str, object: &serde_json::Value) -> Result<usize, String> {
    match object_type {
        "BeaconVote" => match try_parse_beacon_vote(object) {
            Ok(beacon_vote) => Ok(beacon_vote.as_ssz_bytes().len()),
            Err(e) => Err(format!("Failed to parse BeaconVote: {}", e)),
        },
        "PartialSignatureMessage" => match try_parse_partial_signature_message(object) {
            Ok(partial_sig_msg) => Ok(partial_sig_msg.as_ssz_bytes().len()),
            Err(e) => Err(format!("Failed to parse PartialSignatureMessage: {}", e)),
        },
        "PartialSignatureMessages" => match try_parse_partial_signature_messages(object) {
            Ok(partial_sig_msgs) => Ok(partial_sig_msgs.as_ssz_bytes().len()),
            Err(e) => Err(format!("Failed to parse PartialSignatureMessages: {}", e)),
        },
        "SignedSSVMessage" => match try_parse_signed_ssv_message(object) {
            Ok(signed_msg) => Ok(signed_msg.as_ssz_bytes().len()),
            Err(e) => Err(format!("Failed to parse SignedSSVMessage: {}", e)),
        },
        "SSVMessage" => match try_parse_ssv_message(object) {
            Ok(ssv_msg) => Ok(ssv_msg.as_ssz_bytes().len()),
            Err(e) => Err(format!("Failed to parse SSVMessage: {}", e)),
        },
        "qbftMessage" => match try_parse_qbft_message(object) {
            Ok(qbft_msg) => Ok(qbft_msg.as_ssz_bytes().len()),
            Err(e) => Err(format!("Failed to parse QbftMessage: {}", e)),
        },
        "ValidatorConsensusData" => match try_parse_validator_consensus_data(object) {
            Ok(validator_data) => Ok(validator_data.as_ssz_bytes().len()),
            Err(e) => Err(format!("Failed to parse ValidatorConsensusData: {}", e)),
        },
        _ => Err(format!("Unknown object type: {}", object_type)),
    }
}

/// Validate that a max size is within reasonable bounds
fn validate_max_size(object_type: &str, size: usize) -> bool {
    match object_type {
        "BeaconVote" => size <= 1024,              // Should be around 112 bytes
        "PartialSignatureMessage" => size <= 1024, // Should be around 144 bytes
        "PartialSignatureMessages" => size <= 300_000, // Allow up to 300KB for large collections
        "ValidatorConsensusData" => size <= 20_000_000, // Allow up to 20MB (test shows ~10.7MB)
        "qbftMessage" => size <= 10_000_000,       // Allow up to 10MB for large QBFT messages
        "SignedSSVMessage" => size <= 200_000,     // Allow reasonable signed message size
        "SSVMessage" => size <= 200_000,           // Allow reasonable message size
        _ => true,                                 // Unknown types pass validation
    }
}
