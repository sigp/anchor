use serde::Deserialize;

use crate::{SpecTest, SpecTestType, ssv::SsvSpecTestType};

// New duty test for multiple runner duties
#[derive(Debug, Deserialize)]
pub struct MultiStartNewRunnerDutySpecTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Tests", default)]
    pub tests: Vec<serde_json::Value>, // Array of individual test cases

    // Catch all additional fields
    #[serde(flatten)]
    pub additional_fields: std::collections::HashMap<String, serde_json::Value>,
}

impl SpecTest for MultiStartNewRunnerDutySpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running new duty test: {}", self.name);
        println!("Tests: {}", self.tests.len());

        if !self.additional_fields.is_empty() {
            println!(
                "Additional fields: {:?}",
                self.additional_fields.keys().collect::<Vec<_>>()
            );
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Ssv(SsvSpecTestType::NewDuty)
    }
}
