use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType,
    types::types_deserializers::try_parse_validator_consensus_data,
};
use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;

/// Clean, elegant ValidatorConsensusData test with zero boilerplate
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorConsensusDataTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ConsensusData")]
    pub consensus_data: serde_json::Value,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for ValidatorConsensusDataTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No setup needed
    }

    fn run(&self) -> bool {
        let has_expected_error = !self.expected_error.is_empty();

        match self.parse_consensus_data() {
            Ok(consensus_data) => {
                if has_expected_error {
                    // Expected an error but parsing succeeded
                    println!(
                        "❌ Test '{}': Expected error '{}' but parsing succeeded",
                        self.name, self.expected_error
                    );
                    false
                } else {
                    match validate_consensus_data(&consensus_data) {
                        Ok(()) => {
                            println!(
                                "✅ Test '{}': ValidatorConsensusData validation passed",
                                self.name
                            );
                            true
                        }
                        Err(validation_error) => {
                            println!(
                                "❌ Test '{}': Validation failed: {}",
                                self.name, validation_error
                            );
                            false
                        }
                    }
                }
            }
            Err(parse_error) => {
                if has_expected_error && parse_error.contains(&self.expected_error) {
                    println!(
                        "✅ Test '{}': Expected parsing failure: {}",
                        self.name, parse_error
                    );
                    true
                } else {
                    println!(
                        "❌ Test '{}': Unexpected parsing error: {}",
                        self.name, parse_error
                    );
                    false
                }
            }
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ValidatorConsensusData)
    }
}

impl ValidatorConsensusDataTest {
    /// Parse ConsensusData JSON directly to ValidatorConsensusData
    fn parse_consensus_data(&self) -> Result<ValidatorConsensusData, String> {
        try_parse_validator_consensus_data(&self.consensus_data)
    }
}

/// Simple validation - just check that parsing succeeded
fn validate_consensus_data(_data: &ValidatorConsensusData) -> Result<(), String> {
    // For parsing tests, we only care that the data was successfully parsed
    // Business logic validation is handled elsewhere
    Ok(())
}
