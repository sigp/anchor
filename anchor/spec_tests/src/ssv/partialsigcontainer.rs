use serde::Deserialize;

use crate::{SpecTest, SpecTestType, ssv::SsvSpecTestType};

// Partial signature container test
#[derive(Debug, Deserialize)]
pub struct PartialSigContainerTest {
    #[serde(rename = "Name")]
    pub name: String,

    // Catch all additional fields
    #[serde(flatten)]
    pub additional_fields: std::collections::HashMap<String, serde_json::Value>,
}

impl SpecTest for PartialSigContainerTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running partial signature container test: {}", self.name);

        if !self.additional_fields.is_empty() {
            println!(
                "Additional fields: {:?}",
                self.additional_fields.keys().collect::<Vec<_>>()
            );
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Ssv(SsvSpecTestType::PartialSigContainer)
    }
}
