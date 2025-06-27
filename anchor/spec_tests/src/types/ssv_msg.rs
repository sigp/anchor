use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;
use ssv_types::msgid::MessageId;

// 1) SSV message test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SSVMessageTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "MessageIDs")]
    pub message_ids: Vec<MessageId>,

    #[serde(rename = "BelongsToValidator")]
    pub belongs_to_validator: bool,
}

impl SpecTest for SSVMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running SSV message test: {}", self.name);
        // TODO: Implement message ID belonging validation
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SSVMsg)
    }
}

// 2) SSV message encoding test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SSVMessageEncodingTest {
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

impl SpecTest for SSVMessageEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running SSV message encoding test: {}", self.name);
        // TODO: Implement SSV message encoding validation
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SSVMsg)
    }
}
