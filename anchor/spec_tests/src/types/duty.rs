use serde::Deserialize;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};

// Note: we do not have runner roles, just the duty role
// this is just mapping from beacon role => role???
// we dont use this mapping

// Duty mapping test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DutySpecTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "BeaconRole")]
    pub beacon_role: i64,
    #[serde(rename = "RunnerRole")]
    pub runner_role: i64,
}

impl SpecTest for DutySpecTest {
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
        SpecTestType::Types(TypesSpecTestType::Duty)
    }
}
