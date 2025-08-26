use std::collections::HashMap;

use base64::prelude::*;
use qbft::WrappedQbftMessage;
use serde::Deserialize;
use ssv_types::{
    OperatorId,
    consensus::QbftMessage,
    message::{SSVMessage, SignedSSVMessage, SignedSSVMessageError},
};
use ssz::Decode;

use crate::utils::{
    deserializers::{deserialize_base64, deserialize_hex},
    error_mapping::map_signed_message_error,
};

/// Error type for test message conversion
#[derive(Debug, Clone)]
pub enum TestMessageConversionError {
    /// Base64 decode error
    Base64Decode(String),
    /// Invalid signature length
    InvalidSignatureLength { expected: usize, got: usize },
    /// SSZ decode error
    SSZDecode(String),
    /// SignedSSVMessage creation error
    SignedSSVMessage(SignedSSVMessageError),
    /// Multi-signer not allowed for this message type
    MultiSignerNotAllowed,
    /// Missing SSV message
    MissingSSVMessage,
    /// Invalid full data encoding
    InvalidFullData(String),
}

/// Committee member as defined by the spec. Used for parsing
/// and then covnerted into our internal types
#[derive(Debug, Clone, Deserialize)]
pub struct SpecTestCommitteeMember {
    #[serde(rename = "OperatorID")]
    pub operator_id: OperatorId,

    #[serde(rename = "CommitteeID", deserialize_with = "deserialize_hex")]
    pub committee_id: Vec<u8>,

    #[serde(rename = "SSVOperatorPubKey")]
    pub ssv_operator_pub_key: Option<String>,

    #[serde(rename = "FaultyNodes")]
    pub faulty_nodes: u64,

    #[serde(rename = "Committee")]
    pub committee: Option<Vec<SpecTestOperator>>,

    #[serde(rename = "DomainType", deserialize_with = "deserialize_hex")]
    pub domain_type: Vec<u8>,
}

/// Operator from the spec test
#[derive(Debug, Clone, Deserialize)]
pub struct SpecTestOperator {
    #[serde(rename = "OperatorID")]
    pub operator_id: u64,
    #[serde(rename = "SSVOperatorPubKey")]
    pub ssv_operator_pub_key: String,
}

/// Timer state expected after test execution
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectedTimerState {
    #[serde(rename = "Timeouts")]
    pub timeouts: u64,

    #[serde(rename = "Round")]
    pub round: Option<u64>,
}

/// Container for QBFT messages indexed by a key
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageContainer {
    #[serde(rename = "Msgs")]
    pub msgs: HashMap<String, TestSignedSSVMessage>,
}

/// Accepted proposal for the current round
#[derive(Debug, Clone, Deserialize)]
pub struct AcceptedProposal {
    #[serde(rename = "SignedMessage")]
    pub signed_message: TestSignedSSVMessage,

    #[serde(rename = "QBFTMessage")]
    pub qbft_message: QbftMessageData,
}

/// QBFT message data structure
#[derive(Debug, Clone, Deserialize)]
pub struct QbftMessageData {
    #[serde(rename = "MsgType")]
    pub msg_type: u64,

    #[serde(rename = "Height")]
    pub height: u64,

    #[serde(rename = "Round")]
    pub round: u64,

    #[serde(rename = "Identifier", deserialize_with = "deserialize_base64")]
    pub identifier: Vec<u8>,

    #[serde(rename = "Root", deserialize_with = "deserialize_hex")]
    pub root: Vec<u8>,

    #[serde(rename = "DataRound")]
    pub data_round: u64,

    #[serde(rename = "RoundChangeJustification")]
    pub round_change_justification: Vec<serde_json::Value>,

    #[serde(rename = "PrepareJustification")]
    pub prepare_justification: Vec<serde_json::Value>,
}

// Intermediate test-specific SignedSSVMessage that can handle null SSVMessage
#[derive(Debug, Clone, Deserialize)]
pub struct TestSignedSSVMessage {
    #[serde(rename = "Signatures")]
    pub signatures: Vec<String>,

    #[serde(rename = "OperatorIDs")]
    pub operator_ids: Option<Vec<OperatorId>>,

    #[serde(rename = "SSVMessage")]
    pub ssv_message: Option<SSVMessage>,

    #[serde(rename = "FullData")]
    pub full_data: Option<String>,
}

impl TryFrom<TestSignedSSVMessage> for SignedSSVMessage {
    type Error = TestMessageConversionError;

    fn try_from(test_msg: TestSignedSSVMessage) -> Result<Self, Self::Error> {
        // Convert signatures from base64 strings to [u8; 256] arrays
        let mut signatures = Vec::new();
        for sig_str in &test_msg.signatures {
            let sig_bytes = BASE64_STANDARD
                .decode(sig_str.as_bytes())
                .map_err(|e| TestMessageConversionError::Base64Decode(e.to_string()))?;

            if sig_bytes.len() != 256 {
                return Err(TestMessageConversionError::InvalidSignatureLength {
                    expected: 256,
                    got: sig_bytes.len(),
                });
            }

            let mut sig_array = [0u8; 256];
            sig_array.copy_from_slice(&sig_bytes);
            signatures.push(sig_array);
        }

        // Get SSV message or error
        let ssv_message = test_msg
            .ssv_message
            .clone()
            .ok_or(TestMessageConversionError::MissingSSVMessage)?;

        // Decode full_data from base64 string to bytes
        let full_data_bytes = match &test_msg.full_data {
            Some(base64_str) => BASE64_STANDARD
                .decode(base64_str.as_bytes())
                .map_err(|e| TestMessageConversionError::InvalidFullData(e.to_string()))?,
            None => Vec::new(),
        };

        // Create our SignedSSVMessage
        SignedSSVMessage::new(
            signatures,
            test_msg.operator_ids.clone().unwrap_or_default(),
            ssv_message,
            full_data_bytes,
        )
        .map_err(TestMessageConversionError::SignedSSVMessage)
    }
}

impl TestSignedSSVMessage {
    /// Convert to WrappedQbftMessage for processing by core QBFT
    pub fn to_wrapped_qbft_message(&self) -> Result<WrappedQbftMessage, String> {
        // Use conversion to get teh signed ssv message
        let signed_message: SignedSSVMessage = match self.clone().try_into() {
            Ok(msg) => msg,
            Err(TestMessageConversionError::SignedSSVMessage(e)) => {
                let err_string = map_signed_message_error(&e);
                return Err(err_string);
            }
            Err(_) => return Err("Unknown error".to_string()),
        };

        // Valiate the signed message
        if let Err(e) = signed_message.validate() {
            let err_string = map_signed_message_error(&e);
            return Err(err_string);
        }

        // Get the qbft message
        let ssv_message = signed_message.ssv_message();
        let qbft_message = QbftMessage::from_ssz_bytes(ssv_message.data()).unwrap();

        // Create WrappedQbftMessage (we already decoded qbft_message above)
        Ok(WrappedQbftMessage {
            signed_message,
            qbft_message,
        })
    }
}
