use serde::Deserialize;
use ssv_types::consensus::{
    BEACON_ROLE_AGGREGATOR, BEACON_ROLE_ATTESTER, BEACON_ROLE_PROPOSER, BEACON_ROLE_SYNC_COMMITTEE,
    BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION, BEACON_ROLE_VALIDATOR_REGISTRATION,
    BEACON_ROLE_VOLUNTARY_EXIT, BeaconRole,
};
use ssz::{Decode, Encode};

use crate::SpecTest;

/// Mirrors Go's `MapDutyToRunnerRole()`.
fn map_beacon_role_to_duty_role(beacon_role: BeaconRole) -> i32 {
    match beacon_role {
        BEACON_ROLE_ATTESTER | BEACON_ROLE_SYNC_COMMITTEE => 0,
        BEACON_ROLE_PROPOSER => 2,
        BEACON_ROLE_AGGREGATOR | BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION => 6,
        BEACON_ROLE_VALIDATOR_REGISTRATION => 4,
        BEACON_ROLE_VOLUNTARY_EXIT => 5,
        _ => -1,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DutySpecTest {
    name: String,
    beacon_role: u64,
    #[serde(rename = "RunnerRole")]
    expected_duty_role: i32,
}

impl SpecTest for DutySpecTest {
    fn run(&self) -> Result<(), String> {
        let beacon_role = BeaconRole::from_ssz_bytes(&self.beacon_role.as_ssz_bytes())
            .map_err(|e| format!("Invalid beacon role {}: {e:?}", self.beacon_role))?;
        let result = map_beacon_role_to_duty_role(beacon_role);
        if result != self.expected_duty_role {
            return Err(format!(
                "Test '{}': BeaconRole({}) mapped to {result}, expected {}",
                self.name, self.beacon_role, self.expected_duty_role
            ));
        }
        Ok(())
    }
}
