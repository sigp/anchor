use serde::Deserialize;
use ssv_types::message::SignedSSVMessage;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};

// Share test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareTest {
    #[serde(rename = "Name")]
    pub name: String,

    // TODO: Add Share field when type is available
    // #[serde(rename = "Share")]
    // pub share: types::Share,
    #[serde(rename = "Message")]
    pub message: SignedSSVMessage,

    #[serde(rename = "ExpectedHasQuorum")]
    pub expected_has_quorum: bool,

    #[serde(rename = "ExpectedFullCommittee")]
    pub expected_full_committee: bool,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for ShareTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running share test: {}", self.name);
        // TODO: Implement share validation logic
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Share)
    }
}

// Share encoding test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareEncodingTest {
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

impl SpecTest for ShareEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running share encoding test: {}", self.name);
        // TODO: Implement share encoding validation
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Share)
    }
}
