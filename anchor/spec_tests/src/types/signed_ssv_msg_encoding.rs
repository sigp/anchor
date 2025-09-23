use serde::Deserialize;
use ssv_types::message::SignedSSVMessage;
use ssz::{Decode, Encode};

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType, utils::deserializers::deserialize_base64,
};

// Encoding test structure
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SignedSSVMessageEncodingTest {
    #[serde(deserialize_with = "deserialize_base64")]
    pub data: Vec<u8>,
}

impl SpecTest for SignedSSVMessageEncodingTest {
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
