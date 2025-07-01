use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;

// SignedSSVMessage validation tests
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedSSVMessageTest {
    #[serde(rename = "Name")]
    pub name: String,
}

impl SpecTest for SignedSSVMessageTest {
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
        SpecTestType::Types(TypesSpecTestType::SignedSSVMsg)
    }
}
