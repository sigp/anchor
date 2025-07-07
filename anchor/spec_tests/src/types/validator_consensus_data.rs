use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType,
    utils::deserializers::validator_consensus_data_parse::*,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorConsensusDataTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ConsensusData")]
    pub consensus_data: Value,
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
        let consensus_data = match try_parse_validator_consensus_data(&self.consensus_data) {
            Ok(data) => data,
            Err(e) => todo!(),
        };

        // todo!() validate
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ValidatorConsensusData)
    }
}
