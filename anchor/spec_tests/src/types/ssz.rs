use serde::Deserialize;
use ssv_types::consensus::ValidatorConsensusData;
use ssz::Decode;
use tree_hash::TreeHash;
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
        // If we expect an error, any decode failure should be considered success
        if !self.expected_error.is_empty() {
            return self.validate_error_case();
        }

        // Try to validate as different SSZ types
        let result = self.validate_ssz_data();
        if !result {
            eprintln!(
                "SSZ test '{}' failed: could not decode data or validate root",
                self.name
            );
            eprintln!("Data length: {} bytes", self.data.len());
            eprintln!("Expected root: {:?}", self.expected_root);
        }
        result
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Ssz)
    }
}

impl SSZSpecTest {
    /// Validate that decoding fails as expected
    fn validate_error_case(&self) -> bool {
        // Try ValidatorConsensusData first
        if let Ok(cd) = ValidatorConsensusData::from_ssz_bytes(&self.data) {
            // If we successfully decoded but expect an error, try decoding the inner data
            let fork = ForkName::from(cd.version.clone());
            if BeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(&cd.data_ssz, fork).is_err() {
                return true; // Expected error occurred
            }
        }

        // Try BeaconBlock directly for different forks
        for fork in [
            ForkName::Base,
            ForkName::Altair,
            ForkName::Bellatrix,
            ForkName::Capella,
            ForkName::Deneb,
            ForkName::Electra,
        ] {
            if BeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(&self.data, fork).is_err() {
                return true; // Expected error occurred
            }
        }

        false // No error occurred but we expected one
    }

    /// Validate SSZ data and compute hash tree root
    fn validate_ssz_data(&self) -> bool {
        // Try as ValidatorConsensusData first
        if let Ok(cd) = ValidatorConsensusData::from_ssz_bytes(&self.data) {
            return self.validate_consensus_data(&cd);
        }

        // Try as BeaconBlock for different forks
        for fork in [
            ForkName::Capella,
            ForkName::Deneb,
            ForkName::Electra,
            ForkName::Bellatrix,
            ForkName::Altair,
            ForkName::Base,
        ] {
            if let Ok(block) =
                BeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(&self.data, fork)
            {
                return self.validate_beacon_block(&block, fork);
            }
        }

        // If we can't decode as any known type, this is a failure
        false
    }

    /// Validate ValidatorConsensusData and extract relevant hash
    fn validate_consensus_data(&self, cd: &ValidatorConsensusData) -> bool {
        let fork = ForkName::from(cd.version.clone());

        // Try to decode the inner beacon block
        match BeaconBlock::<MainnetEthSpec>::from_ssz_bytes_for_fork(&cd.data_ssz, fork) {
            Ok(block) => self.validate_beacon_block(&block, fork),
            Err(_) => false,
        }
    }

    /// Validate BeaconBlock and extract the appropriate root
    fn validate_beacon_block(&self, block: &BeaconBlock<MainnetEthSpec>, fork: ForkName) -> bool {
        match fork {
            // For Capella and later, extract withdrawals root
            ForkName::Capella | ForkName::Deneb | ForkName::Electra => {
                match block.body().execution_payload() {
                    Ok(payload) => {
                        match payload.withdrawals_root() {
                            Ok(withdrawals_root) => withdrawals_root == self.expected_root,
                            Err(_) => {
                                // Fallback to block root if withdrawals_root fails
                                block.tree_hash_root() == self.expected_root
                            }
                        }
                    }
                    Err(_) => {
                        // Fallback to block root if payload access fails
                        block.tree_hash_root() == self.expected_root
                    }
                }
            }
            // For earlier forks, use block root
            _ => block.tree_hash_root() == self.expected_root,
        }
    }
}
