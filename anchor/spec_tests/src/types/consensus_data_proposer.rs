use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    SpecTest, SpecTestType,
    types::TypesSpecTestType,
    utils::deserializers::type_parse::{
        deserialize_base64_option_to_bytes, deserialize_base64_to_bytes,
        deserialize_bytes_to_hash256,
    },
};

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
        let consensus_data = match ValidatorConsensusData::from_ssz_bytes(&self.data_cd) {
            Ok(data) => data,
            Err(_) => return !self.expected_error.is_empty(),
        };

        // todo!() need block validation logic
        // https://github.com/sigp/anchor/issues/258

        // Compute tree hash root and compare with expected
        let computed_root = consensus_data.tree_hash_root();
        if self.expected_cd_root != computed_root {
            return false;
        }

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
