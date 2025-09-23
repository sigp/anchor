use serde::Deserialize;
use ssv_types::partial_sig::PartialSignatureMessages;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

use crate::{
    SpecTest, SpecTestType,
    types::TypesSpecTestType,
    utils::deserializers::{deserialize_base64, deserialize_bytes_to_hash256},
};

// Encoding test for partial signature messages
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PartialSigMessageEncodingTest {
    #[serde(deserialize_with = "deserialize_base64")]
    pub data: Vec<u8>,
    #[serde(deserialize_with = "deserialize_bytes_to_hash256")]
    pub expected_root: types::Hash256,
}

impl SpecTest for PartialSigMessageEncodingTest {
    fn run(&self) -> bool {
        // Decode the PartialSignatureMessages from the provided data
        let partial_sig_messages = match PartialSignatureMessages::from_ssz_bytes(&self.data) {
            Ok(psm) => psm,
            Err(_) => {
                return false;
            }
        };

        // Compute tree hash root and compare with expected
        let computed_root = partial_sig_messages.tree_hash_root();
        if self.expected_root != computed_root {
            return false;
        }

        // Test roundtrip encoding
        let re_encoded = partial_sig_messages.as_ssz_bytes();
        match PartialSignatureMessages::from_ssz_bytes(&re_encoded) {
            Ok(re_decoded) => {
                if re_decoded != partial_sig_messages {
                    return false;
                }
            }
            Err(_) => {
                return false;
            }
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::PartialSigMessageEncoding)
    }
}
