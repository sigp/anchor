use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::Decode;
use types::{BeaconBlock, ExecPayload, ForkName, Hash256, MainnetEthSpec};

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType, utils::deserializers::type_parse::*,
};

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
