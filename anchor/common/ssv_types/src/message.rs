use std::fmt::{Debug, Formatter};

use crate::{msgid::MessageId, MAX_SIGNATURES};
use base64::prelude::*;
use serde::{de::Error, Deserialize, Deserializer};
use serde_json::Value;
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use ssz_types::VariableList;
use thiserror::Error;
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use types::typenum::{Prod, Sum, U1000, U412, U722};
use types::Hash256;

const QBFT_MSG_TYPE_SIZE: usize = 8;
const HEIGHT_SIZE: usize = 8;
const ROUND_SIZE: usize = 8;
const MAX_NO_JUSTIFICATION_SIZE: usize = 3616;
const MAX1_JUSTIFICATION_SIZE: usize = 50624;
const IDENTIFIER_SIZE: usize = 56; // same as MessageId length
const ROOT_SIZE: usize = 32;

// For partial signatures
const PARTIAL_SIGNATURE_SIZE: usize = 96;
const OPERATOR_ID_SIZE: usize = 8;
const VALIDATOR_INDEX_SIZE: usize = 8;
const SLOT_SIZE: usize = 8;
const PARTIAL_SIG_MSG_TYPE_SIZE: usize = 8;
const MAX_PARTIAL_SIGNATURE_MESSAGES: usize = 1000;
const ENCODING_OVERHEAD_DIVISOR: usize = 20;

const MAX_CONSENSUS_MSG_SIZE: usize = QBFT_MSG_TYPE_SIZE
    + HEIGHT_SIZE
    + ROUND_SIZE
    + IDENTIFIER_SIZE
    + ROOT_SIZE
    + ROUND_SIZE
    + MAX_SIGNATURES * (MAX_NO_JUSTIFICATION_SIZE + MAX1_JUSTIFICATION_SIZE);

const MAX_ENCODED_CONSENSUS_MSG_SIZE: usize =
    MAX_CONSENSUS_MSG_SIZE + (MAX_CONSENSUS_MSG_SIZE / ENCODING_OVERHEAD_DIVISOR) + 4;

const PARTIAL_SIGNATURE_MSG_SIZE: usize =
    PARTIAL_SIGNATURE_SIZE + ROOT_SIZE + OPERATOR_ID_SIZE + VALIDATOR_INDEX_SIZE;

const MAX_PARTIAL_SIGNATURE_MSGS_SIZE: usize = PARTIAL_SIG_MSG_TYPE_SIZE
    + SLOT_SIZE
    + MAX_PARTIAL_SIGNATURE_MESSAGES * PARTIAL_SIGNATURE_MSG_SIZE;

const MAX_ENCODED_PARTIAL_SIGNATURE_SIZE: usize = MAX_PARTIAL_SIGNATURE_MSGS_SIZE
    + (MAX_PARTIAL_SIGNATURE_MSGS_SIZE / ENCODING_OVERHEAD_DIVISOR)
    + 4;

/// SSVMessage.Data max size: 722412 (from Go spec)
/// 722412 = 722 * 1000 + 412 = 722000 + 412
type SSVMessageDataLen = Sum<Prod<U722, U1000>, U412>;

#[cfg(test)]
#[test]
fn ensure_message_size_correct() {
    use typenum::Unsigned;

    assert_eq!(
        SSVMessageDataLen::to_usize(),
        std::cmp::max(
            MAX_ENCODED_PARTIAL_SIGNATURE_SIZE,
            MAX_ENCODED_CONSENSUS_MSG_SIZE
        )
    );
}
/// Defines the types of messages with explicit discriminant values.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
#[repr(u64)]
pub enum MsgType {
    SSVConsensusMsgType = 0,
    SSVPartialSignatureMsgType = 1,
}

impl<'de> Deserialize<'de> for MsgType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        match value {
            0 => Ok(MsgType::SSVConsensusMsgType),
            1 => Ok(MsgType::SSVPartialSignatureMsgType),
            _ => Err(serde::de::Error::custom(format!(
                "Invalid MsgType value: {}",
                value
            ))),
        }
    }
}

impl TreeHash for MsgType {
    fn tree_hash_type() -> TreeHashType {
        TreeHashType::Basic
    }

    fn tree_hash_packed_encoding(&self) -> PackedEncoding {
        let value = self.clone() as u64;
        value.tree_hash_packed_encoding()
    }

    fn tree_hash_packing_factor() -> usize {
        u64::tree_hash_packing_factor()
    }

    fn tree_hash_root(&self) -> Hash256 {
        let value = self.clone() as u64;
        value.tree_hash_root()
    }
}

impl TryFrom<u64> for MsgType {
    type Error = DecodeError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(MsgType::SSVConsensusMsgType),
            1 => Ok(MsgType::SSVPartialSignatureMsgType),
            _ => Err(DecodeError::NoMatchingVariant),
        }
    }
}

const U64_SIZE: usize = 8; // u64 is 8 bytes

impl Encode for MsgType {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let value: u64 = match self {
            MsgType::SSVConsensusMsgType => 0,
            MsgType::SSVPartialSignatureMsgType => 1,
        };
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn ssz_fixed_len() -> usize {
        U64_SIZE
    }

    fn ssz_bytes_len(&self) -> usize {
        U64_SIZE
    }
}

impl Decode for MsgType {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        U64_SIZE
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        u64::from_ssz_bytes(bytes)?.try_into()
    }
}

/// Represents errors that can occur while handling an SSVMessage.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SSVMessageError {
    #[error("SSVMessage data is empty")]
    EmptyData,

    #[error("SSVMessage data too large: got {provided}, max {max}")]
    SSVDataTooBig { provided: usize, max: usize },

    #[error("Wrong domain: got {got}, expected {want}")]
    WrongDomain { got: String, want: String },

    #[error("Signer {got} not in committee: {want:?}")]
    SignerNotInCommittee { got: u64, want: Vec<u64> },
}

/// Represents a bare SSVMessage with a type, ID, and data.
#[derive(Encode, Decode, Clone, PartialEq, Eq, Deserialize, TreeHash)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct SSVMessage {
    #[serde(rename = "MsgType")]
    msg_type: MsgType,

    #[serde(rename = "MsgID")]
    msg_id: MessageId,

    #[serde(rename = "Data")]
    #[serde(deserialize_with = "crate::message::deserialize_base64_message_data")]
    data: VariableList<u8, SSVMessageDataLen>,
}

impl Debug for SSVMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SSVMessage")
            .field("msg_type", &self.msg_type)
            .field("msg_id", &self.msg_id)
            .field("data", &hex::encode(self.data.to_vec()))
            .finish()
    }
}

impl SSVMessage {
    pub fn new(
        msg_type: MsgType,
        msg_id: MessageId,
        data: VariableList<u8, SSVMessageDataLen>,
    ) -> Result<Self, SSVMessageError> {
        let ssv_message = SSVMessage {
            msg_type,
            msg_id,
            data,
        };
        ssv_message.validate()?;
        Ok(ssv_message)
    }

    /// Creates a new `SSVMessage` using a vec instead of a `VariableList`.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - The type of the message.
    /// * `msg_id` - The message ID, showing which duty and validator/committee this belongs to.
    /// * `data` - The message data.
    ///
    /// # Examples
    ///
    /// ```
    /// use ssv_types::{message::{MsgType, SSVMessage}, msgid::MessageId};
    /// let message_id = MessageId::from([0u8; 56]);
    /// let msg = SSVMessage::new_from_vec(MsgType::SSVConsensusMsgType, message_id, vec![1, 2, 3]);
    /// ```
    pub fn new_from_vec(
        msg_type: MsgType,
        msg_id: MessageId,
        data: Vec<u8>,
    ) -> Result<Self, SSVMessageError> {
        let ssv_message = SSVMessage {
            msg_type,
            msg_id,
            data: crate::vec_to_variable_list!(data, SSVMessageError::SSVDataTooBig)?,
        };
        ssv_message.validate()?;
        Ok(ssv_message)
    }

    pub fn validate(&self) -> Result<(), SSVMessageError> {
        if self.data.is_empty() {
            return Err(SSVMessageError::EmptyData);
        }
        match self.msg_type {
            MsgType::SSVConsensusMsgType => {
                if self.data.len() > MAX_ENCODED_CONSENSUS_MSG_SIZE {
                    return Err(SSVMessageError::SSVDataTooBig {
                        provided: self.data.len(),
                        max: MAX_ENCODED_CONSENSUS_MSG_SIZE,
                    });
                }
            }
            MsgType::SSVPartialSignatureMsgType => {
                if self.data.len() > MAX_ENCODED_PARTIAL_SIGNATURE_SIZE {
                    return Err(SSVMessageError::SSVDataTooBig {
                        provided: self.data.len(),
                        max: MAX_ENCODED_PARTIAL_SIGNATURE_SIZE,
                    });
                }
            }
        }
        Ok(())
    }

    /// Returns a reference to the message type.
    pub fn msg_type(&self) -> &MsgType {
        &self.msg_type
    }

    /// Returns a reference to the message ID.
    pub fn msg_id(&self) -> &MessageId {
        &self.msg_id
    }

    /// Returns a reference to the message data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// A testing helping function to create invalid messages.
    #[cfg(test)]
    pub fn new_unvalidated(
        msg_type: MsgType,
        msg_id: MessageId,
        data: VariableList<u8, SSVMessageDataLen>,
    ) -> Self {
        SSVMessage {
            msg_type,
            msg_id,
            data,
        }
    }
}

pub fn deserialize_base64_message_data<'de, D>(
    deserializer: D,
) -> Result<VariableList<u8, SSVMessageDataLen>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;

    match value {
        Value::Null => Ok(VariableList::<u8, SSVMessageDataLen>::new(vec![0]).expect("Valid size")), /* Return empty Vec for null values */
        Value::String(s) => Ok(VariableList::<u8, SSVMessageDataLen>::from(
            BASE64_STANDARD
                .decode(s.as_bytes())
                .map_err(D::Error::custom)?,
        )),
        _ => Err(D::Error::custom("Expected null or a base64 string")),
    }
}

#[cfg(test)]
mod tests {

    use ssz::{Decode, Encode};

    use super::*;

    // Helper functions for building valid test data
    //

    /// Returns a default 56-byte ID array with all zeros.
    fn default_msg_id() -> MessageId {
        [0u8; IDENTIFIER_SIZE].into()
    }

    /// Returns a small, non-empty payload for SSVMessage data.
    fn small_data() -> Vec<u8> {
        vec![0x11, 0x22, 0x33]
    }

    /// Creates a valid, non-empty SSVMessage (ensuring it doesn’t exceed the max size).
    fn valid_ssv_message() -> SSVMessage {
        SSVMessage::new_from_vec(MsgType::SSVConsensusMsgType, default_msg_id(), small_data())
            .expect("Creating a valid SSVMessage must succeed")
    }

    // Tests for MessageId
    //

    #[test]
    fn test_message_id_creation() {
        let id = [1u8; 56];
        let message_id = MessageId::from(id);
        assert_eq!(message_id.as_ref(), &id);
    }

    #[test]
    fn test_message_id_encode_decode() {
        let id = [42u8; 56];
        let message_id = MessageId::from(id);
        let encoded = message_id.as_ssz_bytes();
        assert_eq!(encoded.len(), 56);
        let decoded = MessageId::from_ssz_bytes(&encoded).unwrap();
        assert_eq!(decoded, message_id);
    }

    #[test]
    fn test_message_id_decode_invalid_length() {
        let bytes = vec![0u8; 55]; // One byte short

        let result = MessageId::from_ssz_bytes(&bytes);

        assert!(matches!(
            result,
            Err(DecodeError::InvalidByteLength {
                len: 55,
                expected: 56
            })
        ));
    }

    // Tests for MsgType
    //

    #[test]
    fn test_msgtype_encode_decode() {
        let msg_type = MsgType::SSVConsensusMsgType;
        let encoded = msg_type.as_ssz_bytes();
        assert_eq!(encoded.len(), U64_SIZE);
        let decoded = MsgType::from_ssz_bytes(&encoded).unwrap();
        assert_eq!(decoded, msg_type);

        let msg_type = MsgType::SSVPartialSignatureMsgType;
        let encoded = msg_type.as_ssz_bytes();
        let decoded = MsgType::from_ssz_bytes(&encoded).unwrap();
        assert_eq!(decoded, msg_type);
    }

    #[test]
    fn test_msgtype_decode_invalid_variant() {
        let invalid_value = 2u64.to_le_bytes();

        let result = MsgType::from_ssz_bytes(&invalid_value);

        assert!(matches!(result, Err(DecodeError::NoMatchingVariant)));
    }

    #[test]
    fn test_msgtype_invalid_bytes_length() {
        let bytes = vec![0u8; U64_SIZE - 1]; // One byte short

        let result = MsgType::from_ssz_bytes(&bytes);

        assert!(matches!(
            result,
            Err(DecodeError::InvalidByteLength {
                len: 7,
                expected: 8
            })
        ));
    }

    // Tests for SSVMessage
    //

    /// Checks that a valid SSVMessage is created successfully.
    #[test]
    fn test_ssv_message_valid() {
        let ssv = valid_ssv_message();

        assert!(!ssv.data().is_empty(), "Data should be non-empty");
    }

    /// Checks that empty data triggers `EmptyData` error.
    #[test]
    fn test_ssv_message_empty_data() {
        let result = SSVMessage::new_from_vec(
            MsgType::SSVPartialSignatureMsgType,
            default_msg_id(),
            vec![],
        );

        match result {
            Err(SSVMessageError::EmptyData) => (), // success
            other => panic!("Expected EmptyData, got {other:?}"),
        }
    }

    /// Checks that data exceeding `MAX_CONSENSUS_MSG_SIZE` triggers `SSVDataTooBig`.
    #[test]
    fn test_consensus_message_too_big() {
        let oversized = vec![0u8; MAX_ENCODED_CONSENSUS_MSG_SIZE + 1];

        let result =
            SSVMessage::new_from_vec(MsgType::SSVConsensusMsgType, default_msg_id(), oversized);

        match result {
            Err(SSVMessageError::SSVDataTooBig { provided, max }) => {
                assert_eq!(provided, MAX_ENCODED_CONSENSUS_MSG_SIZE + 1);
                assert_eq!(max, MAX_ENCODED_CONSENSUS_MSG_SIZE);
            }
            other => panic!("Expected SSVDataTooBig, got {other:?}"),
        }
    }

    /// Checks that data exceeding `MAX_PARTIAL_SIGNATURE_MSGS_SIZE` triggers `SSVDataTooBig`.
    #[test]
    fn test_partial_signature_message_too_big() {
        let oversized = vec![0u8; MAX_ENCODED_PARTIAL_SIGNATURE_SIZE + 1];

        let result = SSVMessage::new_from_vec(
            MsgType::SSVPartialSignatureMsgType,
            default_msg_id(),
            oversized,
        );

        match result {
            Err(SSVMessageError::SSVDataTooBig { provided, max }) => {
                assert_eq!(provided, MAX_ENCODED_PARTIAL_SIGNATURE_SIZE + 1);
                assert_eq!(max, MAX_ENCODED_PARTIAL_SIGNATURE_SIZE);
            }
            other => panic!("Expected SSVDataTooBig, got {other:?}"),
        }
    }

    /// Test encoding/decoding a valid SSVMessage.
    #[test]
    fn test_ssv_message_encode_decode() {
        let original = valid_ssv_message();
        let bytes = original.as_ssz_bytes();

        let decoded = SSVMessage::from_ssz_bytes(&bytes);

        assert!(
            decoded.is_ok(),
            "Decoding SSVMessage failed: {:?}",
            decoded.err()
        );

        let decoded = decoded.expect("Should decode successfully");

        assert_eq!(
            decoded, original,
            "Decoded SSVMessage not equal to original"
        );
    }

    #[test]
    fn test_ssvmessage_decode_invalid_length() {
        let bytes = vec![0u8; 56 + 8 + 3 - 1]; // Missing one byte in data

        let result = SSVMessage::from_ssz_bytes(&bytes);

        assert!(result.is_err());
    }
}
