mod committee;
mod msgprocessing;
mod newduty;
mod partialsigcontainer;
mod runnerconstruction;
mod synccommitteeaggregator;
mod valcheck;

use serde::{Deserialize, Deserializer};
use std::fmt;

// Re-export test implementations
pub use committee::*;
pub use msgprocessing::*;
pub use newduty::*;
pub use partialsigcontainer::*;
pub use runnerconstruction::*;
pub use synccommitteeaggregator::*;
pub use valcheck::*;

// SSV-specific test type enumeration
#[derive(Eq, PartialEq, Hash, Debug)]
pub(crate) enum SsvSpecTestType {
    Committee,
    MultiCommittee,
    NewDuty,
    PartialSigContainer,
    RunnerConstruction,
    SyncCommitteeAggregator,
    MsgProcessing,
    MultiMsgProcessing,
    ValCheck,
    MultiValCheck,
}

// Contains specific identifier for the test file matching Go test naming
impl fmt::Display for SsvSpecTestType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SsvSpecTestType::Committee => write!(f, "committee.CommitteeSpecTest"),
            SsvSpecTestType::MultiCommittee => write!(f, "committee.MultiCommitteeSpecTest"),
            SsvSpecTestType::NewDuty => write!(f, "newduty.MultiStartNewRunnerDutySpecTest"),
            SsvSpecTestType::PartialSigContainer => {
                write!(f, "partialsigcontainer.PartialSigContainerTest")
            }
            SsvSpecTestType::RunnerConstruction => {
                write!(f, "runnerconstruction.RunnerConstructionSpecTest")
            }
            SsvSpecTestType::SyncCommitteeAggregator => write!(
                f,
                "synccommitteeaggregator.SyncCommitteeAggregatorProofSpecTest"
            ),
            SsvSpecTestType::MsgProcessing => write!(f, "tests.MsgProcessingSpecTest"),
            SsvSpecTestType::MultiMsgProcessing => write!(f, "tests.MultiMsgProcessingSpecTest"),
            SsvSpecTestType::ValCheck => write!(f, "valcheck.SpecTest"),
            SsvSpecTestType::MultiValCheck => write!(f, "valcheck.MultiSpecTest"),
        }
    }
}

// Custom SSV-specific serde deserializers
pub(crate) mod ssv_deserializers {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use ssv_types::{CommitteeId, OperatorId, ValidatorIndex, domain_type::DomainType};
    use types::{Address, Graffiti, Hash256, PublicKeyBytes};

    // Convert base64 string to RSA public key bytes
    pub(crate) fn deserialize_rsa_public_key<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let base64_string = String::deserialize(deserializer)?;
        STANDARD.decode(&base64_string).map_err(|e| {
            serde::de::Error::custom(format!("Failed to decode RSA public key: {}", e))
        })
    }

    // Convert string to OperatorId
    pub(crate) fn deserialize_operator_id<'de, D>(deserializer: D) -> Result<OperatorId, D::Error>
    where
        D: Deserializer<'de>,
    {
        let id = u64::deserialize(deserializer)?;
        Ok(OperatorId::from(id))
    }

    // Convert string to Slot
    pub(crate) fn deserialize_slot_from_string<'de, D>(
        deserializer: D,
    ) -> Result<types::Slot, D::Error>
    where
        D: Deserializer<'de>,
    {
        let slot_str = String::deserialize(deserializer)?;
        slot_str
            .parse::<u64>()
            .map(types::Slot::new)
            .map_err(|e| serde::de::Error::custom(format!("Failed to parse slot: {}", e)))
    }

    // Convert string to ValidatorIndex
    pub(crate) fn deserialize_validator_index_from_string<'de, D>(
        deserializer: D,
    ) -> Result<ValidatorIndex, D::Error>
    where
        D: Deserializer<'de>,
    {
        let index_str = String::deserialize(deserializer)?;
        index_str.parse::<usize>().map(ValidatorIndex).map_err(|e| {
            serde::de::Error::custom(format!("Failed to parse validator index: {}", e))
        })
    }

    // Convert byte array to CommitteeId (ensures 32-byte length)
    pub(crate) fn deserialize_committee_id<'de, D>(deserializer: D) -> Result<CommitteeId, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = <Vec<u8>>::deserialize(deserializer)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "CommitteeId must be exactly 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut committee_id = [0u8; 32];
        committee_id.copy_from_slice(&bytes);
        Ok(CommitteeId::from(committee_id))
    }

    // Convert 4-byte array to DomainType
    pub(crate) fn deserialize_domain_type<'de, D>(deserializer: D) -> Result<DomainType, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = <[u8; 4]>::deserialize(deserializer)?;
        Ok(DomainType::from(bytes))
    }

    // Convert byte array to PublicKeyBytes (ensures 48-byte length)
    pub(crate) fn deserialize_public_key_bytes<'de, D>(
        deserializer: D,
    ) -> Result<PublicKeyBytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = <Vec<u8>>::deserialize(deserializer)?;
        if bytes.len() != 48 {
            return Err(serde::de::Error::custom(format!(
                "PublicKeyBytes must be exactly 48 bytes, got {}",
                bytes.len()
            )));
        }

        // Use deserialize method directly from bytes
        PublicKeyBytes::deserialize(&bytes)
            .map_err(|e| serde::de::Error::custom(format!("Invalid PublicKeyBytes: {:?}", e)))
    }

    // Convert 20-byte array to Address (Ethereum address)
    pub(crate) fn deserialize_address<'de, D>(deserializer: D) -> Result<Address, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = <[u8; 20]>::deserialize(deserializer)?;
        Ok(Address::from(bytes))
    }

    // Convert base64 string to Graffiti
    pub(crate) fn deserialize_graffiti<'de, D>(deserializer: D) -> Result<Graffiti, D::Error>
    where
        D: Deserializer<'de>,
    {
        let base64_string = String::deserialize(deserializer)?;
        let bytes = STANDARD
            .decode(&base64_string)
            .map_err(|e| serde::de::Error::custom(format!("Failed to decode graffiti: {}", e)))?;

        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "Graffiti must be exactly 32 bytes, got {}",
                bytes.len()
            )));
        }

        let mut graffiti = [0u8; 32];
        graffiti.copy_from_slice(&bytes);
        Ok(Graffiti::from(graffiti))
    }

    // Convert hex string to Hash256
    pub(crate) fn deserialize_hash256_from_hex<'de, D>(deserializer: D) -> Result<Hash256, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex_string = String::deserialize(deserializer)?;
        let bytes = hex::decode(&hex_string)
            .map_err(|e| serde::de::Error::custom(format!("Invalid hex string: {}", e)))?;

        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "Hash256 must be exactly 32 bytes, got {}",
                bytes.len()
            )));
        }

        Ok(Hash256::from_slice(&bytes))
    }

    // Convert optional vector of hex strings to Hash256 vector
    pub(crate) fn deserialize_optional_hash_vec<'de, D>(
        deserializer: D,
    ) -> Result<Option<Vec<Hash256>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt_strings = Option::<Vec<String>>::deserialize(deserializer)?;
        match opt_strings {
            Some(strings) => {
                let mut hashes = Vec::new();
                for hex_string in strings {
                    let bytes = hex::decode(&hex_string).map_err(|e| {
                        serde::de::Error::custom(format!("Invalid hex string: {}", e))
                    })?;

                    if bytes.len() != 32 {
                        return Err(serde::de::Error::custom(format!(
                            "Hash256 must be exactly 32 bytes, got {}",
                            bytes.len()
                        )));
                    }

                    hashes.push(Hash256::from_slice(&bytes));
                }
                Ok(Some(hashes))
            }
            None => Ok(None),
        }
    }

    // Convert hex string to PublicKeyBytes (for "0x..." format)
    pub(crate) fn deserialize_public_key_bytes_from_hex<'de, D>(
        deserializer: D,
    ) -> Result<PublicKeyBytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex_string = String::deserialize(deserializer)?;

        // Remove 0x prefix if present
        let hex_str = if hex_string.starts_with("0x") {
            &hex_string[2..]
        } else {
            &hex_string
        };

        let bytes = hex::decode(hex_str)
            .map_err(|e| serde::de::Error::custom(format!("Invalid hex string: {}", e)))?;

        if bytes.len() != 48 {
            return Err(serde::de::Error::custom(format!(
                "PublicKeyBytes must be exactly 48 bytes, got {}",
                bytes.len()
            )));
        }

        // Use deserialize method directly from bytes
        PublicKeyBytes::deserialize(&bytes)
            .map_err(|e| serde::de::Error::custom(format!("Invalid PublicKeyBytes: {:?}", e)))
    }
}
