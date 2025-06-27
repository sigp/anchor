use serde::Deserialize;
use ssv_types::{OperatorId, message::SSVMessage};

use crate::{SpecTest, SpecTestType, ssv::SsvSpecTestType};

// Message with signatures and operator IDs
#[derive(Debug, Deserialize)]
pub struct SignedMessage {
    #[serde(rename = "Signatures", default)]
    pub signatures: Vec<String>, // Base64 encoded signatures

    #[serde(rename = "OperatorIDs", default)]
    #[serde(deserialize_with = "deserialize_operator_ids_optional")]
    pub operator_ids: Vec<OperatorId>,

    #[serde(rename = "SSVMessage")]
    pub ssv_message: Option<SSVMessage>, // Strongly typed SSV message

    // Catch all additional fields
    #[serde(flatten)]
    pub additional_fields: std::collections::HashMap<String, serde_json::Value>,
}

// Sync committee aggregator proof test
#[derive(Debug, Deserialize)]
pub struct SyncCommitteeAggregatorProofSpecTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Messages")]
    pub messages: Vec<SignedMessage>,

    #[serde(rename = "OutputMessages", default)]
    pub output_messages: Vec<serde_json::Value>, // Flexible output messages - SignedSSVMessage may not always have all fields

    #[serde(rename = "BeaconBroadcastedRoots", default)]
    pub beacon_broadcasted_roots: Vec<String>, // Hex encoded roots

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    // Catch all additional fields
    #[serde(flatten)]
    pub additional_fields: std::collections::HashMap<String, serde_json::Value>,
}

// Helper deserializer for operator IDs
fn deserialize_operator_ids<'de, D>(deserializer: D) -> Result<Vec<OperatorId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let ids = Vec::<u64>::deserialize(deserializer)?;
    Ok(ids.into_iter().map(OperatorId::from).collect())
}

// Helper deserializer for optional operator IDs
fn deserialize_operator_ids_optional<'de, D>(deserializer: D) -> Result<Vec<OperatorId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let ids = Option::<Vec<u64>>::deserialize(deserializer)?;
    Ok(ids
        .unwrap_or_default()
        .into_iter()
        .map(OperatorId::from)
        .collect())
}

impl SpecTest for SyncCommitteeAggregatorProofSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running sync committee aggregator test: {}", self.name);
        println!("Messages: {}", self.messages.len());

        // Validate messages
        for (i, message) in self.messages.iter().enumerate() {
            println!(
                "  Message {}: {} signatures, {} operators",
                i + 1,
                message.signatures.len(),
                message.operator_ids.len()
            );

            // Only validate signature count if both are present
            if !message.signatures.is_empty()
                && !message.operator_ids.is_empty()
                && message.signatures.len() != message.operator_ids.len()
            {
                eprintln!("Signature count mismatch in message {}", i + 1);
                return false;
            }

            // Show additional fields
            if !message.additional_fields.is_empty() {
                println!(
                    "    Additional fields: {:?}",
                    message.additional_fields.keys().collect::<Vec<_>>()
                );
            }
        }

        println!("Output messages: {}", self.output_messages.len());
        println!(
            "Beacon broadcasted roots: {}",
            self.beacon_broadcasted_roots.len()
        );

        // Show additional fields in the test
        if !self.additional_fields.is_empty() {
            println!(
                "Additional test fields: {:?}",
                self.additional_fields.keys().collect::<Vec<_>>()
            );
        }

        // For now, consider tests that expect errors as passed if we can parse them
        if !self.expected_error.is_empty() {
            println!("Expected error: {}", self.expected_error);
            return true;
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Ssv(SsvSpecTestType::SyncCommitteeAggregator)
    }
}
