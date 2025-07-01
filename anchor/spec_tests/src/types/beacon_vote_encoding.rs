use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;
use ssv_types::consensus::BeaconVote;
use ssz::Decode;
use tree_hash::TreeHash;
use types::Hash256;

// BeaconVote encoding test
#[derive(Debug, Deserialize)]
pub struct BeaconVoteEncodingTest {
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

impl SpecTest for BeaconVoteEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        // Decode the BeaconVote from the provided data
        let beacon_vote = match BeaconVote::from_ssz_bytes(&self.data) {
            Ok(bv) => bv,
            Err(e) => {
                println!("Failed to decode BeaconVote: {:?}", e);
                return false;
            }
        };

        // Compute the hash tree root and verify it matches the expected root
        if self.expected_root != beacon_vote.tree_hash_root() {
            return false;
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::BeaconVoteEncoding)
    }
}
