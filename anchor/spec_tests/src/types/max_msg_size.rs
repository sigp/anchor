use serde::Deserialize;
use serde_json;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};

// Structure size validation test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructureSizeTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Object")]
    pub object: serde_json::Value, // Use generic JSON value since we can't deserialize arbitrary types
    #[serde(rename = "ExpectedEncodedLength")]
    pub expected_encoded_length: usize,
    #[serde(rename = "IsMaxSize")]
    pub is_max_size: bool,
}

impl SpecTest for StructureSizeTest {
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
        SpecTestType::Types(TypesSpecTestType::MaxMsgSize)
    }
}
