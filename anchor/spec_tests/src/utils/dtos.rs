//! Raw Data Transfer Objects for Go JSON fixture deserialization.
//!
//! These DTOs bridge Go's JSON serialization format to Anchor's production types
//! without polluting production types with serde annotations.

use serde::Deserialize;
use ssv_types::{
    OperatorId, ValidatorIndex,
    message::{MsgType, SSVMessage},
    msgid::MessageId,
    partial_sig::{PartialSignatureKind, PartialSignatureMessage},
};
use ssz::DecodeError;
use types::Hash256;

use super::deserializers::{
    deserialize_base64, deserialize_hex_message_id, deserialize_hex_option,
    deserialize_partial_signature_kind,
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
