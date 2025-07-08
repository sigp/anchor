use serde::Deserialize;
use ssv_types::partial_sig::PartialSignatureMessages;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType, utils::deserializers::type_parse::*,
};

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
        // No-op
    }

    fn run(&self) -> bool {
        // Decode the PartialSignatureMessages from the provided data
        let partial_sig_messages = match PartialSignatureMessages::from_ssz_bytes(&self.data) {
            Ok(psm) => psm,
            Err(e) => {
                println!("Failed to decode PartialSignatureMessages: {e:?}");
                return false;
            }
        };

        // Compute tree hash root and compare with expected
        let computed_root = partial_sig_messages.tree_hash_root();
        if self.expected_root != computed_root {
            println!(
                "Tree hash root mismatch. Expected: {:?}, Got: {:?}",
                self.expected_root, computed_root
            );
            return false;
        }

        // Test roundtrip encoding
        let re_encoded = partial_sig_messages.as_ssz_bytes();
        match PartialSignatureMessages::from_ssz_bytes(&re_encoded) {
            Ok(re_decoded) => {
                if re_decoded != partial_sig_messages {
                    println!("Roundtrip encoding failed");
                    return false;
                }
            }
            Err(e) => {
                println!("Failed to decode re-encoded data: {e:?}");
                return false;
            }
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::PartialSigMessageEncoding)
    }
}
