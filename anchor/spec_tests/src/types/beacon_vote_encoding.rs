use serde::Deserialize;
use ssv_types::consensus::BeaconVote;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    SpecTest, SpecTestType,
    types::TypesSpecTestType,
    utils::deserializers::{deserialize_base64, deserialize_bytes_to_hash256},
};

// BeaconVote encoding test
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct BeaconVoteEncodingTest {
    #[serde(rename = "Type")]
    pub r#type: Option<String>,
    pub documentation: Option<String>,
    pub name: String,
    #[serde(deserialize_with = "deserialize_base64")]
    pub data: Vec<u8>,
    #[serde(deserialize_with = "deserialize_bytes_to_hash256")]
    pub expected_root: Hash256,
}

impl SpecTest for BeaconVoteEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        // Decode the BeaconVote from the provided data
        let beacon_vote = match BeaconVote::from_ssz_bytes(&self.data) {
            Ok(bv) => bv,
            Err(_) => {
                return false;
            }
        };

        // Compute the hash tree root and verify it matches the expected root
        if self.expected_root != beacon_vote.tree_hash_root() {
            return false;
        }

        // Test round trip encoding
        let re_encoded = beacon_vote.as_ssz_bytes();
        match BeaconVote::from_ssz_bytes(&re_encoded) {
            Ok(re_decoded) => {
                if re_decoded != beacon_vote {
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
        SpecTestType::Types(TypesSpecTestType::BeaconVoteEncoding)
    }
}
