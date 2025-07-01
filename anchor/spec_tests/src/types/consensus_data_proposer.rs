use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;

// THESE ARE NOT EVEN USED/TESTED

// Consensus data proposer test - using existing ValidatorConsensusData from ssv_types
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposerSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ConsensusData")]
    pub consensus_data_json: serde_json::Value,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl ProposerSpecTest {
    // Follow established error matching pattern from other tests
    fn is_matching_error(&self, actual_error: &str, expected_error: &str) -> bool {
        if expected_error.is_empty() {
            false
        } else {
            actual_error.contains(expected_error)
        }
    }
}

impl SpecTest for ProposerSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ConsensusDataProposer)
    }
}
