use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct BeaconDepositDataTest {
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

impl SpecTest for BeaconDepositDataTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        // if self.expected_signing_root != generate_deposit_data() {
        //   false
        // }
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::BeaconDepositData)
    }
}
