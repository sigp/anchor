use serde::{Deserialize, Deserializer, de::Error};
use ssv_types::{
    ValidatorIndex,
    consensus::{
        BeaconRole, DataVersion, ValidatorConsensusData as SSVValidatorConsensusData,
        ValidatorDuty as SSVValidatorDuty,
    },
    message::ValidatorConsensusDataLen,
};
use types::{CommitteeIndex, PublicKeyBytes, Slot, VariableList, typenum::U13};

use crate::{
    SpecTest, SpecTestType,
    types::TypesSpecTestType,
    utils::deserializers::{
        deserialize_base64, deserialize_beacon_role, deserialize_data_version,
        deserialize_hex_public_key, deserialize_string_to_committee_index,
        deserialize_string_to_slot, deserialize_string_to_u64,
        deserialize_string_to_validator_index, deserialize_sync_committee_indices,
    },
};

/// Deserialize VariableList from base64 for DataSSZ
fn deserialize_data_ssz<'de, D>(
    deserializer: D,
) -> Result<VariableList<u8, ValidatorConsensusDataLen>, D::Error>
where
    D: Deserializer<'de>,
{
    let bytes = deserialize_base64(deserializer)?;
    VariableList::new(bytes).map_err(|e| Error::custom(format!("DataSSZ too large: {e:?}")))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ValidatorDuty {
    #[serde(rename = "Type", deserialize_with = "deserialize_beacon_role")]
    pub r#type: BeaconRole,
    #[serde(rename = "PubKey", deserialize_with = "deserialize_hex_public_key")]
    pub pub_key: PublicKeyBytes,
    #[serde(deserialize_with = "deserialize_string_to_slot")]
    pub slot: Slot,
    #[serde(deserialize_with = "deserialize_string_to_validator_index")]
    pub validator_index: ValidatorIndex,
    #[serde(deserialize_with = "deserialize_string_to_committee_index")]
    pub committee_index: CommitteeIndex,
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    pub committee_length: u64,
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    pub committees_at_slot: u64,
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    pub validator_committee_index: u64,
    #[serde(deserialize_with = "deserialize_sync_committee_indices")]
    pub validator_sync_committee_indices: VariableList<u64, U13>,
}

impl ValidatorDuty {
    /// Convert to SSV ValidatorDuty type
    pub fn to_ssv_duty(&self) -> SSVValidatorDuty {
        SSVValidatorDuty {
            r#type: self.r#type,
            pub_key: self.pub_key,
            slot: self.slot,
            validator_index: self.validator_index,
            committee_index: self.committee_index,
            committee_length: self.committee_length,
            committees_at_slot: self.committees_at_slot,
            validator_committee_index: self.validator_committee_index,
            validator_sync_committee_indices: self.validator_sync_committee_indices.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ValidatorConsensusData {
    pub duty: ValidatorDuty,
    #[serde(deserialize_with = "deserialize_data_version")]
    pub version: DataVersion,
    #[serde(rename = "DataSSZ", deserialize_with = "deserialize_data_ssz")]
    pub data_ssz: VariableList<u8, ValidatorConsensusDataLen>,
}

impl ValidatorConsensusData {
    /// Convert to SSV ValidatorConsensusData type
    pub fn to_ssv_consensus_data(&self) -> SSVValidatorConsensusData {
        SSVValidatorConsensusData {
            duty: self.duty.to_ssv_duty(),
            version: self.version,
            data_ssz: self.data_ssz.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ValidatorConsensusDataTest {
    #[serde(rename = "Type")]
    pub r#type: Option<String>,
    pub documentation: Option<String>,
    pub name: String,
    pub consensus_data: ValidatorConsensusData,
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
        // Convert to SSV types
        let _consensus_data = self.consensus_data.clone().to_ssv_consensus_data();

        // todo!() need block validation logic
        // https://github.com/sigp/anchor/issues/258

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ValidatorConsensusData)
    }
}
