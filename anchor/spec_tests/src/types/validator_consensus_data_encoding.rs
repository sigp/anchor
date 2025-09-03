use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    SpecTest, SpecTestType,
    types::TypesSpecTestType,
    utils::deserializers::{deserialize_base64, deserialize_bytes_to_hash256},
};

// Validator consensus data encoding test
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct ValidatorConsensusDataEncodingTest {
    #[serde(rename = "Type")]
    pub r#type: String,
    pub documentation: String,
    pub name: String,
    #[serde(deserialize_with = "deserialize_base64")]
    pub data: Vec<u8>,
    #[serde(deserialize_with = "deserialize_bytes_to_hash256")]
    pub expected_root: Hash256,
}

impl SpecTest for ValidatorConsensusDataEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        // Decode the ValidatorConsensusData from SSZ bytes
        let consensus_data = match ValidatorConsensusData::from_ssz_bytes(&self.data) {
            Ok(data) => data,
            Err(_) => return false,
        };

        // Compute tree hash root and compare with expected
        let computed_root = consensus_data.tree_hash_root();
        if self.expected_root != computed_root {
            return false;
        }

        // Test roundtrip encoding
        let re_encoded = consensus_data.as_ssz_bytes();
        match ValidatorConsensusData::from_ssz_bytes(&re_encoded) {
            Ok(re_decoded) => re_decoded == consensus_data,
            Err(_) => false,
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ValidatorConsensusDataEncoding)
    }
}
