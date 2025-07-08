use serde::Deserialize;
use ssv_types::message::SignedSSVMessage;
use ssz::{Decode, Encode};

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType, utils::deserializers::type_parse::*,
};

// Encoding test structure
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedSSVMessageEncodingTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Data", deserialize_with = "deserialize_base64_to_bytes")]
    pub data: Vec<u8>,
}

impl SpecTest for SignedSSVMessageEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        let signed_message = match SignedSSVMessage::from_ssz_bytes(&self.data) {
            Ok(msg) => msg,
            Err(_) => return false,
        };

        // Test roundtrip encoding
        let re_encoded = signed_message.as_ssz_bytes();
        match SignedSSVMessage::from_ssz_bytes(&re_encoded) {
            Ok(re_decoded) => re_decoded == signed_message,
            Err(_) => false,
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SignedSSVMsgEncoding)
    }
}
