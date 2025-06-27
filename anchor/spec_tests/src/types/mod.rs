mod beacon;
mod beacon_vote_encoding;
mod committee_member;
mod consensus_data_proposer;
mod duty;
mod encryption;
mod max_msg_size;
mod partial_sig_message;
mod partial_sig_message_encoding;
mod share_encoding;
mod signed_ssv_msg;
mod ssv_msg;
mod ssv_msg_encoding;
mod ssz;
mod validator_consensus_data;
mod validator_consensus_data_encoding;

use serde::{Deserialize, Deserializer};
use std::fmt;

// Re-export test implementations
pub use beacon::*;
pub use beacon_vote_encoding::*;
pub use committee_member::*;
pub use consensus_data_proposer::*;
pub use duty::*;
pub use encryption::*;
pub use max_msg_size::*;
pub use partial_sig_message::*;
pub use partial_sig_message_encoding::*;
pub use share_encoding::*;
pub use signed_ssv_msg::*;
pub use ssv_msg::*;
pub use ssv_msg_encoding::*;
pub use ssz::*;
pub use validator_consensus_data::*;
pub use validator_consensus_data_encoding::*;

// Types-specific test type enumeration
#[derive(Eq, PartialEq, Hash)]
pub(crate) enum TypesSpecTestType {
    Beacon,
    BeaconVoteEncoding,
    CommitteeMember,
    ConsensusDataProposer,
    Duty,
    Encryption,
    MaxMsgSize,
    PartialSigMessage,
    PartialSigMessageEncoding,
    ShareEncoding,
    SignedSSVMsg,
    SSVMsg,
    SSVMsgEncoding,
    SSZ,
    ValidatorConsensusData,
    ValidatorConsensusDataEncoding,
}

impl TypesSpecTestType {
    // Determine if this is an encoding test
    pub fn is_encoding(&self) -> bool {
        match self {
            TypesSpecTestType::ShareEncoding
            | TypesSpecTestType::PartialSigMessageEncoding
            | TypesSpecTestType::SSVMsgEncoding
            | TypesSpecTestType::ValidatorConsensusDataEncoding => true,
            _ => false,
        }
    }
}

// Contains specific identifier for the test file matching Go test naming
impl fmt::Display for TypesSpecTestType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TypesSpecTestType::Beacon => write!(f, "beacon"),
            TypesSpecTestType::BeaconVoteEncoding => write!(f, "beaconvote"),
            TypesSpecTestType::CommitteeMember => write!(f, "committeemember"),
            TypesSpecTestType::ConsensusDataProposer => write!(f, "consensusdataproposer"),
            TypesSpecTestType::Duty => write!(f, "duty"),
            TypesSpecTestType::Encryption => write!(f, "encryption"),
            TypesSpecTestType::MaxMsgSize => write!(f, "maxmsgsize"),
            TypesSpecTestType::PartialSigMessage => write!(f, "partialsigmessage"),
            TypesSpecTestType::PartialSigMessageEncoding => write!(f, "partialsigmessage"),
            TypesSpecTestType::ShareEncoding => write!(f, "share"),
            TypesSpecTestType::SignedSSVMsg => write!(f, "signedssvmsg"),
            TypesSpecTestType::SSVMsg => write!(f, "ssvmsg"),
            TypesSpecTestType::SSVMsgEncoding => write!(f, "ssvmsg"),
            TypesSpecTestType::SSZ => write!(f, "ssz"),
            TypesSpecTestType::ValidatorConsensusData => write!(f, "validatorconsensusdata"),
            TypesSpecTestType::ValidatorConsensusDataEncoding => {
                write!(f, "validatorconsensusdata")
            }
        }
    }
}

// Custom Types-specific serde deserializers
pub(crate) mod types_deserializers {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use types::Hash256;

    // Convert base64 strings to byte arrays for RSA public keys
    pub(crate) fn deserialize_rsa_public_keys<'de, D>(
        deserializer: D,
    ) -> Result<Vec<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let base64_strings = <Vec<String>>::deserialize(deserializer)?;
        let mut byte_arrays = Vec::new();

        for base64_string in base64_strings {
            let bytes = STANDARD.decode(&base64_string).map_err(|e| {
                serde::de::Error::custom(format!("Failed to decode base64 string: {}", e))
            })?;
            byte_arrays.push(bytes);
        }

        Ok(byte_arrays)
    }

    // Convert base64 string to bytes for data fields
    pub(crate) fn deserialize_base64_to_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let base64_string = String::deserialize(deserializer)?;
        STANDARD
            .decode(&base64_string)
            .map_err(|e| serde::de::Error::custom(format!("Failed to decode base64 string: {}", e)))
    }

    // Convert byte array to Hash256 for expected roots
    pub(crate) fn deserialize_bytes_to_hash256<'de, D>(deserializer: D) -> Result<Hash256, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = <Vec<u8>>::deserialize(deserializer)?;

        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "Expected 32 bytes for Hash256, got {}",
                bytes.len()
            )));
        }

        Ok(Hash256::from_slice(&bytes))
    }

    // Convert array of hex strings to array of 32-byte arrays for multiple roots
    pub(crate) fn deserialize_hex_array_to_hash256_array<'de, D>(
        deserializer: D,
    ) -> Result<Vec<[u8; 32]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex_strings = <Vec<String>>::deserialize(deserializer)?;
        let mut hash_arrays = Vec::new();

        for hex_string in hex_strings {
            let bytes = hex::decode(&hex_string).map_err(|e| {
                serde::de::Error::custom(format!("Failed to decode hex string: {}", e))
            })?;

            if bytes.len() != 32 {
                return Err(serde::de::Error::custom(format!(
                    "Expected 32 bytes for hash, got {}",
                    bytes.len()
                )));
            }

            let mut hash_array = [0u8; 32];
            hash_array.copy_from_slice(&bytes);
            hash_arrays.push(hash_array);
        }

        Ok(hash_arrays)
    }
}
