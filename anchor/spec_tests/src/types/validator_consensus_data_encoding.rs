use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::Decode;
use tree_hash::TreeHash;
use types::Hash256;

// Validator consensus data encoding test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorConsensusDataEncodingTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Data", deserialize_with = "deserialize_base64_to_bytes")]
    pub data: Vec<u8>,
    #[serde(
        rename = "ExpectedRoot",
        deserialize_with = "deserialize_bytes_to_hash256"
    )]
    pub expected_root: Hash256,
}

impl SpecTest for ValidatorConsensusDataEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        // Decode the ValidatorConsensusData from SSZ bytes
        let consensus_data = match ValidatorConsensusData::from_ssz_bytes(&self.data) {
            Ok(data) => data,
            Err(e) => {
                println!("Failed to decode ValidatorConsensusData: {:?}", e);
                return false;
            }
        };

        // Verify the tree hash root matches expected
        if self.expected_root != consensus_data.tree_hash_root() {
            return false;
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ValidatorConsensusDataEncoding)
    }
}
