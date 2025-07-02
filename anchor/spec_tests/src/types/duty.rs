use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;

// Notes:
// This maps a beacon role to a runenr role which is an implementation detail
// Can skip

#[derive(Debug, Deserialize)]
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
        // No-op
    }

    fn run(&self) -> bool {
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Duty)
    }
}
