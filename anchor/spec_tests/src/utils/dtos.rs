//! Raw Data Transfer Objects for Go JSON fixture deserialization.
//!
//! These DTOs bridge Go's JSON serialization format to Anchor's production types
//! without polluting production types with serde annotations.

use std::str::FromStr;

use serde::Deserialize;
use ssv_types::{
    OperatorId, ValidatorIndex,
    consensus::{BeaconRole, DataVersion, ProposerConsensusData, ValidatorDuty},
    message::{MsgType, SSVMessage},
    msgid::MessageId,
    partial_sig::{PartialSignatureKind, PartialSignatureMessage},
};
use ssz::{Decode, DecodeError, Encode};
use ssz_types::VariableList;
use types::{ForkName, Hash256, Slot};

use super::deserializers::{
    deserialize_base64, deserialize_base64_or_empty, deserialize_hex_message_id,
    deserialize_hex_option, deserialize_partial_signature_kind,
};

/// DTO for `SSVMessage`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawSSVMessage {
    msg_type: u64,
    #[serde(rename = "MsgID", deserialize_with = "deserialize_hex_message_id")]
    msg_id: MessageId,
    #[serde(rename = "Data", deserialize_with = "deserialize_base64")]
    data: Vec<u8>,
}

impl TryFrom<&RawSSVMessage> for SSVMessage {
    type Error = String;

    fn try_from(msg: &RawSSVMessage) -> Result<Self, String> {
        let msg_type = MsgType::try_from(msg.msg_type)
            .map_err(|e: DecodeError| format!("Invalid MsgType value {}: {e:?}", msg.msg_type))?;
        SSVMessage::new(msg_type, msg.msg_id.clone(), msg.data.clone())
            .map_err(|e| format!("Invalid SSVMessage: {e}"))
    }
}

/// DTO for `PartialSignatureMessage`. Handles error fixtures with invalid BLS data.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawPartialSignatureMessage {
    #[serde(deserialize_with = "deserialize_hex_option", default)]
    pub partial_signature: Option<Vec<u8>>,
    pub signer: u64,
    #[serde(deserialize_with = "deserialize_hex_option", default)]
    pub signing_root: Option<Vec<u8>>,
    #[serde(default)]
    pub validator_index: Option<String>,
}

impl TryFrom<&RawPartialSignatureMessage> for PartialSignatureMessage {
    type Error = String;

    fn try_from(m: &RawPartialSignatureMessage) -> Result<Self, String> {
        let sig_bytes = m
            .partial_signature
            .as_ref()
            .ok_or("Missing partial_signature")?;
        let partial_signature = bls::Signature::deserialize(sig_bytes)
            .map_err(|e| format!("Invalid BLS signature: {e:?}"))?;

        let root_bytes = m.signing_root.as_ref().ok_or("Missing signing_root")?;
        if root_bytes.len() != 32 {
            return Err(format!(
                "Invalid signing_root length: expected 32, got {}",
                root_bytes.len()
            ));
        }
        let signing_root = Hash256::from_slice(root_bytes);

        let validator_index = m
            .validator_index
            .as_ref()
            .ok_or("Missing validator_index")?
            .parse::<usize>()
            .map_err(|e| format!("Invalid validator_index: {e}"))?;

        Ok(PartialSignatureMessage {
            partial_signature,
            signing_root,
            signer: OperatorId(m.signer),
            validator_index: ValidatorIndex(validator_index),
        })
    }
}

/// DTO for `PartialSignatureMessages`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawPartialSignatureMessages {
    #[serde(
        rename = "Type",
        deserialize_with = "deserialize_partial_signature_kind"
    )]
    pub kind: PartialSignatureKind,
    pub slot: String,
    pub messages: Vec<RawPartialSignatureMessage>,
}

/// DTO for Go's `Duty` object inside `ConsensusData`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawDuty {
    #[serde(rename = "Type")]
    pub duty_type: u64,
    pub pub_key: String,
    pub slot: String,
    pub validator_index: String,
    pub committee_index: u64,
    pub committee_length: u64,
    pub committees_at_slot: u64,
    pub validator_committee_index: u64,
    #[serde(default)]
    pub validator_sync_committee_indices: Option<Vec<u64>>,
}

impl TryFrom<&RawDuty> for ValidatorDuty {
    type Error = String;

    fn try_from(raw: &RawDuty) -> Result<Self, String> {
        let role = BeaconRole::from_ssz_bytes(&raw.duty_type.as_ssz_bytes())
            .map_err(|e| format!("Invalid duty type {}: {e:?}", raw.duty_type))?;

        let pub_key = bls::PublicKeyBytes::from_str(&raw.pub_key)
            .map_err(|e| format!("Invalid pub_key: {e:?}"))?;

        let slot = raw
            .slot
            .parse::<u64>()
            .map(Slot::new)
            .map_err(|e| format!("Invalid slot: {e}"))?;

        let validator_index = raw
            .validator_index
            .parse::<usize>()
            .map(ValidatorIndex)
            .map_err(|e| format!("Invalid validator_index: {e}"))?;

        let sync_indices: Vec<u64> = raw
            .validator_sync_committee_indices
            .clone()
            .unwrap_or_default();
        let validator_sync_committee_indices = VariableList::new(sync_indices)
            .map_err(|_| "validator_sync_committee_indices exceeds max length")?;

        Ok(ValidatorDuty {
            r#type: role,
            pub_key,
            slot,
            validator_index,
            committee_index: raw.committee_index,
            committee_length: raw.committee_length,
            committees_at_slot: raw.committees_at_slot,
            validator_committee_index: raw.validator_committee_index,
            validator_sync_committee_indices,
        })
    }
}

/// DTO for Go's `ConsensusData` fixture (proposer variant).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RawConsensusData {
    pub duty: RawDuty,
    pub version: String,
    #[serde(
        rename = "DataSSZ",
        deserialize_with = "deserialize_base64_or_empty",
        default
    )]
    pub data_ssz: Vec<u8>,
}

impl TryFrom<&RawConsensusData> for ProposerConsensusData {
    type Error = String;

    fn try_from(raw: &RawConsensusData) -> Result<Self, String> {
        let duty = ValidatorDuty::try_from(&raw.duty)?;

        let fork = ForkName::from_str(&raw.version)
            .map_err(|e| format!("Invalid version '{}': {e}", raw.version))?;

        let data_ssz =
            VariableList::new(raw.data_ssz.clone()).map_err(|_| "data_ssz exceeds max length")?;

        Ok(ProposerConsensusData {
            duty,
            version: DataVersion::from(fork),
            data_ssz,
        })
    }
}
