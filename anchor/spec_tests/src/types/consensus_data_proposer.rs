use crate::utils::deserializers::{
    decode_consensus_data_from_ssz, extract_and_validate_block_data, matches_expected_error,
    try_parse_validator_consensus_data,
};
use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use base64::Engine;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use tree_hash::TreeHash;
use types::Hash256;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsensusDataProposerTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Blinded")]
    pub blinded: bool,
    #[serde(rename = "DataCd", deserialize_with = "deserialize_base64_option")]
    pub data_cd: Option<Vec<u8>>,
    #[serde(rename = "DataBlk", deserialize_with = "deserialize_base64_option")]
    pub data_blk: Option<Vec<u8>>,
    #[serde(rename = "ExpectedBlkRoot")]
    pub expected_blk_root: [u8; 32],
    #[serde(rename = "ExpectedCdRoot")]
    pub expected_cd_root: [u8; 32],
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

fn deserialize_base64_option<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        Some(s) => base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map(Some)
            .map_err(D::Error::custom),
        None => Ok(None),
    }
}

impl SpecTest for ConsensusDataProposerTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        // Handle tests that should have error conditions
        if !self.expected_error.is_empty() {
            return self.validate_error_case();
        }

        // Handle success cases - validate everything
        self.validate_success_case()
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ConsensusDataProposer)
    }
}

impl ConsensusDataProposerTest {
    fn validate_error_case(&self) -> bool {
        // For error cases, we expect the validation process to fail with a specific error
        let data_cd = match &self.data_cd {
            Some(data) => data,
            None => {
                // No consensus data provided - this itself might be the error condition
                return true;
            }
        };

        // Try to decode consensus data - this should fail for error test cases
        let consensus_data = match self.decode_consensus_data(data_cd) {
            Ok(data) => data,
            Err(err) => {
                // Check if the error matches what we expect
                return matches_expected_error(&self.expected_error, &err);
            }
        };

        // If decoding succeeded but we expected an error, try block extraction
        match extract_and_validate_block_data(&consensus_data, self.blinded) {
            Ok(_) => {
                // Validation succeeded but we expected an error - this is a test failure
                eprintln!(
                    "Test '{}': Expected error '{}' but validation succeeded",
                    self.name, self.expected_error
                );
                false
            }
            Err(err) => {
                // Check if this error matches what we expect
                matches_expected_error(&self.expected_error, &err)
            }
        }
    }

    fn validate_success_case(&self) -> bool {
        // Get consensus data
        let data_cd = match &self.data_cd {
            Some(data) => data,
            None => {
                eprintln!(
                    "Test '{}': Missing consensus data for success case",
                    self.name
                );
                return false;
            }
        };

        // Decode consensus data
        let consensus_data = match self.decode_consensus_data(data_cd) {
            Ok(data) => data,
            Err(err) => {
                eprintln!(
                    "Test '{}': Failed to decode consensus data: {}",
                    self.name, err
                );
                return false;
            }
        };

        // Validate consensus data root
        let calculated_cd_root = consensus_data.tree_hash_root();
        let expected_cd_root = Hash256::from_slice(&self.expected_cd_root);
        if calculated_cd_root != expected_cd_root {
            eprintln!(
                "Test '{}': Consensus data root mismatch. Expected: {:?}, Got: {:?}",
                self.name, expected_cd_root, calculated_cd_root
            );
            return false;
        }

        // Extract and validate block data
        let calculated_block_root =
            match extract_and_validate_block_data(&consensus_data, self.blinded) {
                Ok(root) => root,
                Err(err) => {
                    eprintln!(
                        "Test '{}': Block data extraction failed: {}",
                        self.name, err
                    );
                    return false;
                }
            };

        // Validate block root
        let expected_blk_root = Hash256::from_slice(&self.expected_blk_root);
        if calculated_block_root != expected_blk_root {
            eprintln!(
                "Test '{}': Block root mismatch. Expected: {:?}, Got: {:?}",
                self.name, expected_blk_root, calculated_block_root
            );
            return false;
        }

        // Validate block data marshalling if provided
        if let Some(expected_block_data) = &self.data_blk {
            let block_data_vec: Vec<u8> = consensus_data.data_ssz.as_ref().to_vec();
            if block_data_vec != *expected_block_data {
                eprintln!(
                    "Test '{}': Block data marshalling validation failed",
                    self.name
                );
                return false;
            }
        }

        true
    }

    fn decode_consensus_data(
        &self,
        data: &[u8],
    ) -> Result<ssv_types::consensus::ValidatorConsensusData, String> {
        // Try to use the existing deserializer first
        if let Ok(consensus_data) = try_parse_validator_consensus_data(&serde_json::Value::String(
            base64::engine::general_purpose::STANDARD.encode(data),
        )) {
            return Ok(consensus_data);
        }

        // Fallback to direct SSZ decoding
        decode_consensus_data_from_ssz(data)
    }
}
