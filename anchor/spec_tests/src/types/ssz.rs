use serde::Deserialize;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};

// SSZ test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SSZSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    // TODO: Add specific SSZ test fields based on Go implementation
}

impl SpecTest for SSZSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running SSZ test: {}", self.name);
        // TODO: Implement SSZ validation (e.g., withdrawals marshaling)
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SSZ)
    }
}
