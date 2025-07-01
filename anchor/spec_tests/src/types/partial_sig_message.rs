use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;

// Partial signature message test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialSigMsgSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
}

impl SpecTest for PartialSigMsgSpecTest {
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
        SpecTestType::Types(TypesSpecTestType::PartialSigMessage)
    }
}
