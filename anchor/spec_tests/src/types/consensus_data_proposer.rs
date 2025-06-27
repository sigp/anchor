use serde::Deserialize;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};

// Consensus data proposer test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposerSpecTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
    // TODO: Add consensus data fields when types are available
}

impl SpecTest for ProposerSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running consensus data proposer test: {}", self.name);
        // TODO: Implement proposer consensus data validation
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ConsensusDataProposer)
    }
}
