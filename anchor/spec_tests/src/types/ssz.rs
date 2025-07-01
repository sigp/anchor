use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::Decode;
use types::{BeaconBlock, BlindedBeaconBlock, ExecPayload, ForkName, Hash256, MainnetEthSpec};

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
        println!("🔍 Running SSZ test: {}", self.name);

        // Parse the ValidatorConsensusData from the test data
        let cd = match ValidatorConsensusData::from_ssz_bytes(&self.data) {
            Ok(cd) => cd,
            Err(e) => {
                if !self.expected_error.is_empty() {
                    println!("✅ Expected error occurred during parsing: {:?}", e);
                    return true;
                } else {
                    println!("❌ Unexpected error during parsing: {:?}", e);
                    return false;
                }
            }
        };

        // Convert DataVersion to ForkName for deserialization
        let fork = ForkName::from(cd.version);

        // Try to deserialize as full BeaconBlock first, then BlindedBeaconBlock
        let withdrawals_root = match BeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(
            &cd.data_ssz,
            fork,
        ) {
            Ok(full_block) => {
                // Extract withdrawals from full block based on fork version
                match fork {
                    ForkName::Capella | ForkName::Deneb | ForkName::Electra => {
                        match full_block.body().execution_payload() {
                            Ok(payload) => {
                                // For full blocks, get the withdrawals root
                                match payload.withdrawals_root() {
                                    Ok(root) => root,
                                    Err(e) => {
                                        println!(
                                            "❌ Failed to get withdrawals root from full block: {:?}",
                                            e
                                        );
                                        return false;
                                    }
                                }
                            }
                            Err(e) => {
                                println!(
                                    "❌ Failed to get execution payload from full block: {:?}",
                                    e
                                );
                                return false;
                            }
                        }
                    }
                    _ => {
                        println!("❌ Unsupported fork version for withdrawals: {:?}", fork);
                        return false;
                    }
                }
            }
            Err(e) => {
                println!("{:?}", e);
                // Try parsing as BlindedBeaconBlock if full block parsing fails
                match BlindedBeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(
                    &cd.data_ssz,
                    fork,
                ) {
                    Ok(blinded_block) => {
                        // Extract withdrawals from blinded block based on fork version
                        match fork {
                            ForkName::Capella | ForkName::Deneb | ForkName::Electra => {
                                match blinded_block.body().execution_payload() {
                                    Ok(payload) => match payload.withdrawals_root() {
                                        Ok(root) => root,
                                        Err(e) => {
                                            println!("❌ Failed to get withdrawals root: {:?}", e);
                                            return false;
                                        }
                                    },
                                    Err(e) => {
                                        println!(
                                            "❌ Failed to get execution payload from blinded block: {:?}",
                                            e
                                        );
                                        return false;
                                    }
                                }
                            }
                            _ => {
                                println!("❌ Unsupported fork version for withdrawals: {:?}", fork);
                                return false;
                            }
                        }
                    }
                    Err(e) => {
                        if !self.expected_error.is_empty() {
                            println!("✅ Expected error occurred during block parsing: {:?}", e);
                            return true;
                        } else {
                            println!(
                                "❌ Failed to parse both full and blinded block data: {:?}",
                                e
                            );
                            return false;
                        }
                    }
                }
            }
        };

        // Compare the computed root with the expected root
        if withdrawals_root == self.expected_root {
            println!("✅ Withdrawals tree hash root matches expected value");
            true
        } else {
            println!(
                "❌ Withdrawals tree hash root mismatch. Expected: {:?}, Got: {:?}",
                self.expected_root, withdrawals_root
            );
            false
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SSZ)
    }
}
