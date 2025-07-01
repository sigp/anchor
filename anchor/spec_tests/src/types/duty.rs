use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DutySpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "BeaconRole")]
    pub beacon_role: i64,
    #[serde(rename = "RunnerRole")]
    pub runner_role: i64,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for DutySpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Duty)
    }
}
