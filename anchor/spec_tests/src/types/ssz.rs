use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::Decode;
use types::{BeaconBlock, ExecPayload, ForkName, Hash256, MainnetEthSpec};

use crate::{
    SpecTest, SpecTestType,
    types::TypesSpecTestType,
    utils::deserializers::{deserialize_base64, deserialize_hex_hash256},
};

/// SSZ test for validating SSZ encoding and decoding operations
///
/// This test validates SSZ marshaling and hash tree root calculations,
/// particularly for Capella withdrawals and other consensus objects.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct SSZSpecTest {
    /// The name of the test case
    pub name: String,
    /// The type of test being performed (e.g. "SSZ: validation of SSZ encoding and decoding")
    #[serde(rename = "Type")]
    pub test_type: Option<String>,
    /// Documentation describing what the test does
    pub documentation: Option<String>,
    /// Base64 encoded SSZ data to decode and validate
    #[serde(deserialize_with = "deserialize_base64")]
    pub data: Vec<u8>,
    /// The expected hash tree root as hex string
    #[serde(deserialize_with = "deserialize_hex_hash256")]
    pub expected_root: Hash256,
    /// Expected error message (empty string if no error expected)
    pub expected_error: String,
}

impl SpecTest for SSZSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No setup required
    }

    fn run(&self) -> bool {
        let cd = match ValidatorConsensusData::from_ssz_bytes(&self.data) {
            Ok(cd) => cd,
            Err(_) => return !self.expected_error.is_empty(),
        };

        // Convert DataVersion to ForkName for deserialization
        let fork = ForkName::from(cd.version);

        // Try to deserialize as full BeaconBlock first
        let withdrawals_root =
            match BeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(&cd.data_ssz, fork) {
                Ok(full_block) => match fork {
                    ForkName::Capella | ForkName::Deneb | ForkName::Electra => {
                        match full_block.body().execution_payload() {
                            Ok(payload) => match payload.withdrawals_root() {
                                Ok(root) => root,
                                Err(_) => return false,
                            },
                            Err(_) => return false,
                        }
                    }
                    _ => return false,
                },
                Err(_) => return false,
            };

        withdrawals_root == self.expected_root
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Ssz)
    }
}
