use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::{Decode, Encode};

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType, utils::deserializers::deserialize_base64,
};

#[derive(Debug, Deserialize)]
pub struct ConsensusDataProposerTest {
    #[serde(rename = "DataCd", deserialize_with = "deserialize_base64")]
    pub data_cd: Vec<u8>,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for ConsensusDataProposerTest {
    fn run(&self) -> bool {
        let consensus_data = match ValidatorConsensusData::from_ssz_bytes(&self.data_cd) {
            Ok(data) => data,
            Err(_) => {
                let has_error = !self.expected_error.is_empty();
                if !has_error {
                    return false;
                }
                return true;
            }
        };

        // Test roundtrip encoding
        let re_encoded = consensus_data.as_ssz_bytes();
        if re_encoded != self.data_cd {
            return false;
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ConsensusDataProposer)
    }
}
