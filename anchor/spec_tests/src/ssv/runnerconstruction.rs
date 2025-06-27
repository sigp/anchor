use serde::Deserialize;

use crate::{SpecTest, SpecTestType, ssv::SsvSpecTestType};

// Runner construction test
#[derive(Debug, Deserialize)]
pub struct RunnerConstructionSpecTest {
    #[serde(rename = "Name")]
    pub name: String,

    // Catch all additional fields
    #[serde(flatten)]
    pub additional_fields: std::collections::HashMap<String, serde_json::Value>,
}

impl SpecTest for RunnerConstructionSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running runner construction test: {}", self.name);

        if !self.additional_fields.is_empty() {
            println!(
                "Additional fields: {:?}",
                self.additional_fields.keys().collect::<Vec<_>>()
            );
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Ssv(SsvSpecTestType::RunnerConstruction)
    }
}
