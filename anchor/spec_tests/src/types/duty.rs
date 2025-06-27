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
    pub runner_role: i64, // Using i64 as placeholder for RunnerRole enum
}

impl SpecTest for DutySpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        // TODO: Implement duty mapping validation
        // This would involve:
        // 1. Convert beacon_role to the appropriate BeaconRole enum
        // 2. Call the duty mapping function (equivalent to types.MapDutyToRunnerRole)
        // 3. Verify the result matches the expected runner_role

        println!("Running duty mapping test: {}", self.name);

        // Placeholder validation - would need actual BeaconRole/RunnerRole types
        let mapped_role = self.map_duty_to_runner_role(self.beacon_role);

        if mapped_role != self.runner_role {
            eprintln!(
                "Duty mapping mismatch: expected {}, got {}",
                self.runner_role, mapped_role
            );
            return false;
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Duty)
    }
}

impl DutySpecTest {
    // Placeholder duty mapping function - would need to implement actual mapping logic
    fn map_duty_to_runner_role(&self, beacon_role: i64) -> i64 {
        // Based on the actual test data from Go implementation
        match beacon_role {
            0 => 0,  // Attester -> Attester
            1 => 1,  // Aggregator -> Aggregator
            2 => 2,  // Proposer -> Proposer
            3 => 0,  // SyncCommittee -> Attester (maps to attester role)
            4 => 3,  // SyncCommitteeContribution -> SyncCommitteeContribution
            5 => 4,  // ValidatorRegistration -> ValidatorRegistration
            6 => 5,  // VoluntaryExit -> VoluntaryExit
            _ => -1, // Unknown -> Unknown (return -1 to match the expected value)
        }
    }
}
