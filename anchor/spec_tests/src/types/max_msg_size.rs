use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;
use serde_json;

// we require a new parsing structure
// Structure size validation test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaxMsgSizeTest {
    #[serde(rename = "Name")]
    pub name: String,
    // Use generic Json value since object differs for test
    #[serde(rename = "Object")]
    pub object: serde_json::Value,
    #[serde(rename = "ExpectedEncodedLength")]
    pub expected_encoded_length: usize,
    #[serde(rename = "IsMaxSize")]
    pub is_max_size: bool,
}

impl SpecTest for MaxMsgSizeTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::MaxMsgSize)
    }
}
