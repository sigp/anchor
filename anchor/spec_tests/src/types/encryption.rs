use serde::Deserialize;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};

// Encryption test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncryptionSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "SKPem", deserialize_with = "deserialize_base64_to_bytes")]
    pub sk_pem: Vec<u8>,
    #[serde(rename = "PKPem", deserialize_with = "deserialize_base64_to_bytes")]
    pub pk_pem: Vec<u8>,
    #[serde(rename = "PlainText", deserialize_with = "deserialize_base64_to_bytes")]
    pub plain_text: Vec<u8>,
}

impl SpecTest for EncryptionSpecTest {
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
        SpecTestType::Types(TypesSpecTestType::Encryption)
    }
}
