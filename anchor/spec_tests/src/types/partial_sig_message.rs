use serde::Deserialize;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};

// Partial signature message test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsgSpecTest {
    #[serde(rename = "Name")]
    pub name: String,

    // TODO: Add PartialSignatureMessages field when type is available
    // #[serde(rename = "Messages")]
    // pub messages: Vec<types::PartialSignatureMessages>,
    #[serde(rename = "EncodedMessages")]
    pub encoded_messages: Vec<Vec<u8>>,

    #[serde(
        rename = "ExpectedRoots",
        deserialize_with = "deserialize_hex_array_to_hash256_array"
    )]
    pub expected_roots: Vec<[u8; 32]>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for MsgSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running partial signature message test: {}", self.name);
        // TODO: Implement partial signature message validation
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::PartialSigMessage)
    }
}

// Encoding test for partial signature messages
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialSigMessageEncodingTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Data", deserialize_with = "deserialize_base64_to_bytes")]
    pub data: Vec<u8>,

    #[serde(
        rename = "ExpectedRoot",
        deserialize_with = "deserialize_bytes_to_hash256"
    )]
    pub expected_root: types::Hash256,
}

impl SpecTest for PartialSigMessageEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!(
            "Running partial signature message encoding test: {}",
            self.name
        );
        // TODO: Implement partial signature message encoding validation
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::PartialSigMessage)
    }
}
