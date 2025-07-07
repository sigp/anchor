use crate::utils::deserializers::type_parse::{
    deserialize_base64_option_to_bytes, deserialize_base64_to_bytes, deserialize_bytes_to_hash256,
};
use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::Decode;
use types::Hash256;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsensusDataProposerTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Blinded")]
    pub blinded: bool,
    #[serde(rename = "DataCd", deserialize_with = "deserialize_base64_to_bytes")]
    pub data_cd: Vec<u8>,
    #[serde(
        rename = "DataBlk",
        deserialize_with = "deserialize_base64_option_to_bytes"
    )]
    pub data_blk: Option<Vec<u8>>,
    #[serde(
        rename = "ExpectedBlkRoot",
        deserialize_with = "deserialize_bytes_to_hash256"
    )]
    pub expected_blk_root: Hash256,
    #[serde(
        rename = "ExpectedCdRoot",
        deserialize_with = "deserialize_bytes_to_hash256"
    )]
    pub expected_cd_root: Hash256,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for ConsensusDataProposerTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        let _consensus_data = match ValidatorConsensusData::from_ssz_bytes(&self.data_cd) {
            Ok(data) => data,
            Err(_err) => {
                // todo!() check error
                return true;
            }
        };

        if self.blinded {
            // todo!(). Need to parse block
        } else {
            // todo!(). Need to parse block
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ConsensusDataProposer)
    }
}
