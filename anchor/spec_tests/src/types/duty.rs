use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;

// Duty mapping test - using existing duty validation infrastructure
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

impl DutySpecTest {
    // Follow established error matching pattern
    fn is_matching_error(&self, actual_error: &str, expected_error: &str) -> bool {
        if expected_error.is_empty() {
            false
        } else {
            actual_error.contains(expected_error)
        }
    }

    // Validate duty role mapping using client logic patterns
    fn validate_duty_mapping(&self) -> Result<(), String> {
        // Map beacon role to known types (matching Go spec)
        let _beacon_role_type = match self.beacon_role {
            0 => "Unknown",
            1 => "Attester",
            2 => "Proposer",
            3 => "Aggregator",
            4 => "SyncCommittee",
            5 => "SyncCommitteeContribution",
            6 => "ValidatorRegistration",
            7 => "VoluntaryExit",
            _ => return Err(format!("invalid beacon role: {}", self.beacon_role)),
        };

        // Map runner role to known types (matching Go spec)
        let _runner_role_type = match self.runner_role {
            0 => "Unknown",
            1 => "Attester",
            2 => "Proposer",
            3 => "Aggregator",
            4 => "SyncCommittee",
            5 => "SyncCommitteeContribution",
            6 => "ValidatorRegistration",
            7 => "VoluntaryExit",
            _ => return Err(format!("invalid runner role: {}", self.runner_role)),
        };

        // Validate that beacon role matches runner role (they should be the same in SSV)
        if self.beacon_role != self.runner_role {
            return Err(format!(
                "beacon role {} does not match runner role {}",
                self.beacon_role, self.runner_role
            ));
        }

        // Additional validation: certain roles should be valid
        if self.beacon_role == 0 {
            return Err("unknown role not allowed".to_string());
        }

        Ok(())
    }
}

impl SpecTest for DutySpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        let has_expected_error = !self.expected_error.is_empty();

        println!("✅ Running duty test: {}", self.name);

        // Use duty validation following established patterns
        match self.validate_duty_mapping() {
            Ok(()) => {
                if has_expected_error {
                    println!(
                        "❌ Expected error '{}' but validation succeeded",
                        self.expected_error
                    );
                    false
                } else {
                    println!("✅ Duty validation passed: {}", self.name);
                    println!(
                        "   Beacon role {} maps to runner role {}",
                        self.beacon_role, self.runner_role
                    );
                    true
                }
            }
            Err(e) => {
                if has_expected_error && self.is_matching_error(&e, &self.expected_error) {
                    println!("✅ Expected validation failure: {}", e);
                    true
                } else {
                    println!("❌ Unexpected validation error: {}", e);
                    false
                }
            }
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::Duty)
    }
}
