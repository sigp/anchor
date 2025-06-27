use serde::Deserialize;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};

// Validator consensus data test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorConsensusDataTest {
    #[serde(rename = "Name")]
    pub name: String,

    // TODO: Add ValidatorConsensusData field when type is available
    // #[serde(rename = "ConsensusData")]
    // pub consensus_data: types::ValidatorConsensusData,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for ValidatorConsensusDataTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running validator consensus data test: {}", self.name);
        // TODO: Implement validator consensus data validation
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ValidatorConsensusData)
    }
}

// Validator consensus data encoding test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorConsensusDataEncodingTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Data", deserialize_with = "deserialize_base64_to_bytes")]
    pub data: Vec<u8>,

    #[serde(
        rename = "ExpectedRoot",
        deserialize_with = "deserialize_bytes_to_hash256"
    )]
    pub expected_root: types::Hash256,
}

impl SpecTest for ValidatorConsensusDataEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!(
            "Running validator consensus data encoding test: {}",
            self.name
        );
        // TODO: Implement validator consensus data encoding validation
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ValidatorConsensusData)
    }
}
