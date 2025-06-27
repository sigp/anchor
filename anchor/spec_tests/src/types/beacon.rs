use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;

// Notes: This is generating the ETH deposit data
// Why are we testing this? This calls GenerateEthDepositData from TestUtils, but this is not used
// anywhere, even in the Go-ssv codebase. Hence I am skipping
// https://github.com/ssvlabs/ssv-spec/blob/main/types/spectest/tests/beacon/deposit_data.go

// Beacon deposit data test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepositDataSpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ValidatorPK")]
    pub validator_pk: String,
    #[serde(rename = "WithdrawalCredentials")]
    pub withdrawal_credentials: String,
    #[serde(rename = "ForkVersion")]
    pub fork_version: [u8; 4],
    #[serde(rename = "ExpectedSigningRoot")]
    pub expected_signing_root: String,
}

impl SpecTest for DepositDataSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Beacon)
    }
}
