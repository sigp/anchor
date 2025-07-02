use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;

// THESE ARE NOT EVEN USED/TESTED

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsensusDataProposerTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ConsensusData")]
    pub consensus_data_json: serde_json::Value,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for ConsensusDataProposerTest {
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
