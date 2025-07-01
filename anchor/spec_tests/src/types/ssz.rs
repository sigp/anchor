use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::Decode;
use types::Hash256;

// SSZ test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SSZSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Data", deserialize_with = "deserialize_base64_to_bytes")]
    pub data: Vec<u8>,
    #[serde(
        rename = "ExpectedRoot",
        deserialize_with = "deserialize_bytes_to_hash256"
    )]
    pub expected_root: Hash256,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for SSZSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        let _consensus_data = match ValidatorConsensusData::from_ssz_bytes(&self.data) {
            Ok(bv) => bv,
            Err(e) => {
                println!("Failed to decode Consensus data: {:?}", e);
                return false;
            }
        };

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SSZ)
    }
}
