use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;
use ssv_types::consensus::BeaconVote;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

// Notes: The test file and the output comparison are the exact same
// instead, they use a hardcoded "testing" beacon vote that they encode and
// compare to the output. what is the point of the test file then???
// test file: https://github.com/ssvlabs/ssv-spec/blob/main/types/spectest/generate/tests/beaconvote.EncodingTest_beacon_vote_encoding.json
// output file: https://github.com/ssvlabs/ssv-spec/blob/main/types/spectest/generate/state_comparison/beaconvote_EncodingTest/beacon%20vote%20encoding.json
// the test being run: https://github.com/ssvlabs/ssv-spec/blob/main/types/spectest/tests/beaconvote/beacon_vote_encoding.go#L10
// the hardcoded beacon vote: https://github.com/ssvlabs/ssv-spec/blob/4faf15cc6598254f2b4602b094f08cc7aa3c74ef/types/testingutils/beacon_vote.go#L13

// BeaconVote encoding test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
        // Setup any required test state
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

        // Re-encode the BeaconVote and verify it matches the original data
        let re_encoded = beacon_vote.as_ssz_bytes();
        if re_encoded != self.data {
            println!("Re-encoded data does not match original data");
            println!("Original: {:?}", self.data);
            println!("Re-encoded: {:?}", re_encoded);
            return false;
        }

        // Compute the hash tree root and verify it matches the expected root
        let computed_root = beacon_vote.tree_hash_root();
        if computed_root != self.expected_root {
            println!("Computed root does not match expected root");
            println!("Expected: {:?}", self.expected_root);
            println!("Computed: {:?}", computed_root);
            return false;
        }

        println!("BeaconVote encoding test '{}' passed", self.name);
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::BeaconVote)
    }
}
