use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use base64::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use ssv_types::ValidatorIndex;
use ssv_types::consensus::{
    BEACON_ROLE_AGGREGATOR, BEACON_ROLE_ATTESTER, BEACON_ROLE_PROPOSER, BEACON_ROLE_SYNC_COMMITTEE,
    BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION, BEACON_ROLE_VALIDATOR_REGISTRATION,
    BEACON_ROLE_VOLUNTARY_EXIT, BeaconRole, DataVersion, ValidatorConsensusData,
    ValidatorConsensusDataLen, ValidatorDuty,
};
use types::{CommitteeIndex, ForkName, PublicKeyBytes, Slot, VariableList, typenum::U13};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorConsensusDataTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ConsensusData")]
    pub consensus_data: Value,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for ValidatorConsensusDataTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No setup needed
    }

    fn run(&self) -> bool {
        match self.parse_consensus_data() {
            Ok(_consensus_data) => {
                // todo!() validation
                true
            }
            Err(parse_error) => {
                println!("{:?}", parse_error);
                if parse_error == "unknown duty role" {
                    true // This is expected for invalid duty types
                } else {
                    false // Unexpected parsing error
                }
            }
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ValidatorConsensusData)
    }
}

impl ValidatorConsensusDataTest {
    fn parse_consensus_data(&self) -> Result<ValidatorConsensusData, String> {
        let obj = self.consensus_data.as_object().ok_or("Expected object")?;

        // Parse Duty object
        let duty_obj = obj
            .get("Duty")
            .and_then(|v| v.as_object())
            .ok_or("Missing or invalid Duty")?;

        let duty = ValidatorDuty {
            r#type: self.parse_beacon_role(duty_obj.get("Type"))?,
            pub_key: self.parse_public_key(duty_obj.get("PubKey"))?,
            slot: self.parse_slot(duty_obj.get("Slot"))?,
            validator_index: self.parse_validator_index(duty_obj.get("ValidatorIndex"))?,
            committee_index: self.parse_committee_index(duty_obj.get("CommitteeIndex"))?,
            committee_length: self.parse_u64(duty_obj.get("CommitteeLength"), "CommitteeLength")?,
            committees_at_slot: self
                .parse_u64(duty_obj.get("CommitteesAtSlot"), "CommitteesAtSlot")?,
            validator_committee_index: self.parse_u64(
                duty_obj.get("ValidatorCommitteeIndex"),
                "ValidatorCommitteeIndex",
            )?,
            validator_sync_committee_indices: self
                .parse_sync_committee_indices(duty_obj.get("ValidatorSyncCommitteeIndices"))?,
        };

        // Parse Version
        let version = self.parse_data_version(obj.get("Version"))?;

        // Parse DataSSZ
        let data_ssz = self.parse_data_ssz(obj.get("DataSSZ"))?;

        Ok(ValidatorConsensusData {
            duty,
            version,
            data_ssz,
        })
    }

    // Parsing helper functions
    fn parse_beacon_role(&self, value: Option<&serde_json::Value>) -> Result<BeaconRole, String> {
        let num = value
            .and_then(|v| v.as_u64())
            .ok_or("Missing or invalid Type")?;
        match num {
            0 => Ok(BEACON_ROLE_ATTESTER),
            1 => Ok(BEACON_ROLE_AGGREGATOR),
            2 => Ok(BEACON_ROLE_PROPOSER),
            3 => Ok(BEACON_ROLE_SYNC_COMMITTEE),
            4 => Ok(BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION),
            5 => Ok(BEACON_ROLE_VALIDATOR_REGISTRATION),
            6 => Ok(BEACON_ROLE_VOLUNTARY_EXIT),
            _ => Err("unknown duty role".to_string()),
        }
    }

    fn parse_public_key(
        &self,
        value: Option<&serde_json::Value>,
    ) -> Result<PublicKeyBytes, String> {
        let hex_str = value
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid PubKey")?;
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);

        // Ensure the hex string has the 0x prefix for parsing
        let hex_with_prefix = if hex_str.starts_with("0x") {
            hex_str.to_string()
        } else {
            format!("0x{}", hex_str)
        };

        hex_with_prefix
            .parse()
            .map_err(|e| format!("Invalid public key: {}", e))
    }

    fn parse_slot(&self, value: Option<&serde_json::Value>) -> Result<Slot, String> {
        let slot_str = value
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid Slot")?;
        let slot_num: u64 = slot_str
            .parse()
            .map_err(|e| format!("Invalid slot: {}", e))?;
        Ok(Slot::new(slot_num))
    }

    fn parse_validator_index(
        &self,
        value: Option<&serde_json::Value>,
    ) -> Result<ValidatorIndex, String> {
        let index_str = value
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid ValidatorIndex")?;
        let index_num: usize = index_str
            .parse()
            .map_err(|e| format!("Invalid validator index: {}", e))?;
        Ok(ValidatorIndex(index_num))
    }

    fn parse_committee_index(
        &self,
        value: Option<&serde_json::Value>,
    ) -> Result<CommitteeIndex, String> {
        let num = value
            .and_then(|v| v.as_u64())
            .ok_or("Missing or invalid CommitteeIndex")?;
        Ok(CommitteeIndex::from(num))
    }

    fn parse_u64(
        &self,
        value: Option<&serde_json::Value>,
        field_name: &str,
    ) -> Result<u64, String> {
        value
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("Missing or invalid {}", field_name))
    }

    fn parse_sync_committee_indices(
        &self,
        value: Option<&serde_json::Value>,
    ) -> Result<VariableList<u64, U13>, String> {
        match value {
            Some(arr) if !arr.is_null() => {
                let indices: Result<Vec<u64>, _> = arr
                    .as_array()
                    .ok_or("ValidatorSyncCommitteeIndices must be array")?
                    .iter()
                    .map(|v| v.as_u64().ok_or("Invalid sync committee index"))
                    .collect();

                let indices = indices?;
                VariableList::new(indices)
                    .map_err(|_| "Too many sync committee indices".to_string())
            }
            _ => Ok(VariableList::empty()),
        }
    }

    fn parse_data_version(&self, value: Option<&serde_json::Value>) -> Result<DataVersion, String> {
        let version_str = value
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid Version")?;

        // Map version strings to DataVersion (which wraps ForkName)
        let fork_name = match version_str {
            "phase0" => ForkName::Base,
            "altair" => ForkName::Altair,
            "bellatrix" => ForkName::Bellatrix,
            "capella" => ForkName::Capella,
            "deneb" => ForkName::Deneb,
            "electra" => ForkName::Electra,
            _ => return Err(format!("unknown version: {}", version_str)),
        };
        Ok(DataVersion::from(fork_name))
    }

    fn parse_data_ssz(
        &self,
        value: Option<&serde_json::Value>,
    ) -> Result<VariableList<u8, ValidatorConsensusDataLen>, String> {
        let data_str = value
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid DataSSZ")?;

        if data_str.is_empty() {
            return Ok(VariableList::empty());
        }

        let decoded = BASE64_STANDARD
            .decode(data_str)
            .map_err(|_| "Failed to decode base64 DataSSZ")?;

        VariableList::new(decoded).map_err(|_| "DataSSZ too large".to_string())
    }
}
