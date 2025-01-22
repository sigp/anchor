use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use std::fmt;

const MESSAGE_ID_LEN: usize = 56;

/// Represents a unique Message ID consisting of 56 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageID([u8; MESSAGE_ID_LEN]);

impl MessageID {
    /// Creates a new `MessageID` if the provided array is exactly 56 bytes.
    ///
    /// # Arguments
    ///
    /// * `id` - A 56-byte array representing the message ID.
    ///
    /// # Examples
    ///
    /// ```
    /// use network::types::ssv_message::MessageID;
    /// let id = [0u8; 56];
    /// let message_id = MessageID::new(id);
    /// ```
    pub fn new(id: [u8; MESSAGE_ID_LEN]) -> Self {
        MessageID(id)
    }

    /// Returns a reference to the underlying 56-byte array.
    pub fn as_bytes(&self) -> &[u8; MESSAGE_ID_LEN] {
        &self.0
    }
}

impl Encode for MessageID {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.0);
    }

    fn ssz_fixed_len() -> usize {
        MESSAGE_ID_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        MESSAGE_ID_LEN
    }
}

impl Decode for MessageID {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        MESSAGE_ID_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != MESSAGE_ID_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: MESSAGE_ID_LEN,
            });
        }
        let mut id = [0u8; MESSAGE_ID_LEN];
        id.copy_from_slice(bytes);
        Ok(MessageID(id))
    }
}

impl fmt::Display for MessageID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex_str = hex::encode(self.0);
        write!(f, "MessageID({})", hex_str)
    }
}

/// Defines the types of messages with explicit discriminant values.
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(u64)]
pub enum MsgType {
    SSVConsensusMsgType = 0,
    SSVPartialSignatureMsgType = 1,
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
        if bytes.len() != U64_SIZE {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: U64_SIZE,
            });
        }
        let value = u64::from_le_bytes(bytes.try_into().unwrap());
        value.try_into()
    }
}

/// Represents an Operator ID as a 64-bit unsigned integer.
pub type OperatorID = u64;

/// Represents an SSV Message with type, ID, and data.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq)]
pub struct SSVMessage {
    msg_type: MsgType,
    msg_id: MessageID, // Fixed-size [u8; 56]
    data: Vec<u8>,     // Variable-length byte array
}

impl SSVMessage {
    /// Creates a new `SSVMessage`.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - The type of the message.
    /// * `msg_id` - The unique message ID.
    /// * `data` - The message data.
    ///
    /// # Examples
    ///
    /// ```
    /// use network::types::ssv_message::{SSVMessage, MsgType, MessageID};
    /// let message_id = MessageID::new([0u8; 56]);
    /// let msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![1, 2, 3]);
    /// ```
    pub fn new(msg_type: MsgType, msg_id: MessageID, data: Vec<u8>) -> Self {
        SSVMessage {
            msg_type,
            msg_id,
            data,
        }
    }

    /// Returns a reference to the message type.
    pub fn msg_type(&self) -> &MsgType {
        &self.msg_type
    }

    /// Returns a reference to the message ID.
    pub fn msg_id(&self) -> &MessageID {
        &self.msg_id
    }

    /// Returns a reference to the message data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// Represents a signed SSV Message with signatures, operator IDs, the message itself, and full data.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq)]
pub struct SignedSSVMessage {
    signatures: Vec<Vec<u8>>, // Vec of Vec<u8>, max 13 elements, each up to 256 bytes
    operator_ids: Vec<OperatorID>, // Vec of OperatorID (u64), max 13 elements
    ssv_message: SSVMessage,  // SSVMessage: Required field
    full_data: Vec<u8>,       // Variable-length byte array, max 4,194,532 bytes
}

impl SignedSSVMessage {
    /// Maximum allowed number of signatures and operator IDs.
    pub const MAX_SIGNATURES: usize = 13;
    /// Maximum allowed length for each signature in bytes.
    pub const MAX_SIGNATURE_LENGTH: usize = 256;
    /// Maximum allowed length for `full_data` in bytes.
    pub const MAX_FULL_DATA_LENGTH: usize = 4_194_532;

    /// Creates a new `SignedSSVMessage` after validating constraints.
    ///
    /// # Arguments
    ///
    /// * `signatures` - A vector of signatures, each up to 256 bytes.
    /// * `operator_ids` - A vector of operator IDs, maximum 13 elements.
    /// * `ssv_message` - The SSV message.
    /// * `full_data` - Full data, up to 4,194,532 bytes.
    ///
    /// # Errors
    ///
    /// Returns an `SSVMessageError` if any constraints are violated.
    ///
    /// # Examples
    ///
    /// ```
    /// use network::types::ssv_message::{SignedSSVMessage, SSVMessage, MsgType, MessageID};
    /// let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, MessageID::new([0u8; 56]), vec![1,2,3]);
    /// let signed_msg = SignedSSVMessage::new(vec![vec![0; 256]], vec![1], ssv_msg, vec![4,5,6]).unwrap();
    /// ```
    pub fn new(
        signatures: Vec<Vec<u8>>,
        operator_ids: Vec<OperatorID>,
        ssv_message: SSVMessage,
        full_data: Vec<u8>,
    ) -> Result<Self, SSVMessageError> {
        if signatures.len() > Self::MAX_SIGNATURES {
            return Err(SSVMessageError::TooManySignatures {
                provided: signatures.len(),
                max: Self::MAX_SIGNATURES,
            });
        }

        for (i, sig) in signatures.iter().enumerate() {
            if sig.len() > Self::MAX_SIGNATURE_LENGTH {
                return Err(SSVMessageError::SignatureTooLong {
                    index: i,
                    length: sig.len(),
                    max: Self::MAX_SIGNATURE_LENGTH,
                });
            }
        }

        if operator_ids.len() > Self::MAX_SIGNATURES {
            return Err(SSVMessageError::TooManyOperatorIDs {
                provided: operator_ids.len(),
                max: Self::MAX_SIGNATURES,
            });
        }

        if full_data.len() > Self::MAX_FULL_DATA_LENGTH {
            return Err(SSVMessageError::FullDataTooLong {
                length: full_data.len(),
                max: Self::MAX_FULL_DATA_LENGTH,
            });
        }

        Ok(SignedSSVMessage {
            signatures,
            operator_ids,
            ssv_message,
            full_data,
        })
    }

    /// Returns a reference to the signatures.
    pub fn signatures(&self) -> &Vec<Vec<u8>> {
        &self.signatures
    }

    /// Returns a reference to the operator IDs.
    pub fn operator_ids(&self) -> &Vec<OperatorID> {
        &self.operator_ids
    }

    /// Returns a reference to the SSV message.
    pub fn ssv_message(&self) -> &SSVMessage {
        &self.ssv_message
    }

    /// Returns a reference to the full data.
    pub fn full_data(&self) -> &[u8] {
        &self.full_data
    }
}

/// Represents errors that can occur while creating or processing `SignedSSVMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SSVMessageError {
    /// Exceeded the maximum number of signatures.
    TooManySignatures { provided: usize, max: usize },
    /// A signature exceeds the maximum allowed length.
    SignatureTooLong {
        index: usize,
        length: usize,
        max: usize,
    },
    /// Exceeded the maximum number of operator IDs.
    TooManyOperatorIDs { provided: usize, max: usize },
    /// `full_data` exceeds the maximum allowed length.
    FullDataTooLong { length: usize, max: usize },
}

impl fmt::Display for SSVMessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SSVMessageError::TooManySignatures { provided, max } => {
                write!(
                    f,
                    "Too many signatures: provided {}, maximum allowed is {}.",
                    provided, max
                )
            }
            SSVMessageError::SignatureTooLong { index, length, max } => {
                write!(
                    f,
                    "Signature at index {} is too long: {} bytes, maximum allowed is {} bytes.",
                    index, length, max
                )
            }
            SSVMessageError::TooManyOperatorIDs { provided, max } => {
                write!(
                    f,
                    "Too many operator IDs: provided {}, maximum allowed is {}.",
                    provided, max
                )
            }
            SSVMessageError::FullDataTooLong { length, max } => {
                write!(
                    f,
                    "Full data is too long: {} bytes, maximum allowed is {} bytes.",
                    length, max
                )
            }
        }
    }
}

impl std::error::Error for SSVMessageError {}

#[cfg(test)]
mod tests {
    use super::*;
    use ssz::{Decode, Encode};

    #[test]
    fn test_message_id_creation() {
        let id = [1u8; 56];
        let message_id = MessageID::new(id);
        assert_eq!(message_id.as_bytes(), &id);
    }

    #[test]
    fn test_message_id_display() {
        let id = [0xABu8; 56];
        let message_id = MessageID::new(id);
        let display = format!("{}", message_id);
        assert_eq!(display, format!("MessageID({})", "ab".repeat(56)));
    }

    #[test]
    fn test_message_id_encode_decode() {
        let id = [42u8; 56];
        let message_id = MessageID::new(id);
        let encoded = message_id.as_ssz_bytes();
        assert_eq!(encoded.len(), 56);
        let decoded = MessageID::from_ssz_bytes(&encoded).unwrap();
        assert_eq!(decoded, message_id);
    }

    #[test]
    fn test_message_id_decode_invalid_length() {
        let bytes = vec![0u8; 55]; // One byte short
        let result = MessageID::from_ssz_bytes(&bytes);
        assert!(matches!(
            result,
            Err(DecodeError::InvalidByteLength {
                len: 55,
                expected: 56
            })
        ));
    }

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
    fn test_ssv_message_encode_decode() {
        let message_id = MessageID::new([7u8; 56]);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            message_id.clone(),
            vec![10, 20, 30],
        );
        let encoded = ssv_msg.as_ssz_bytes();
        let decoded = SSVMessage::from_ssz_bytes(&encoded).unwrap();
        assert_eq!(decoded, ssv_msg);
    }

    #[test]
    fn test_signed_ssv_message_creation_valid() {
        let message_id = MessageID::new([0u8; 56]);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            message_id,
            vec![1, 2, 3],
        );

        let signatures = vec![vec![0u8; 256], vec![1u8; 100]];
        let operator_ids = vec![1, 2];
        let full_data = vec![255u8; 4_194_532];

        let signed_msg = SignedSSVMessage::new(
            signatures.clone(),
            operator_ids.clone(),
            ssv_msg.clone(),
            full_data.clone(),
        );

        assert!(signed_msg.is_ok());

        let signed_msg = signed_msg.unwrap();
        assert_eq!(signed_msg.signatures(), &signatures);
        assert_eq!(signed_msg.operator_ids(), &operator_ids);
        assert_eq!(signed_msg.ssv_message(), &ssv_msg);
        assert_eq!(signed_msg.full_data(), &full_data);
    }

    #[test]
    fn test_signed_ssv_message_creation_too_many_signatures() {
        let message_id = MessageID::new([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);

        let signatures = vec![vec![0u8; 256]; 14]; // Exceeds max of 13
        let operator_ids = vec![1; 13];
        let full_data = vec![];

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(SSVMessageError::TooManySignatures {
                provided: 14,
                max: 13
            })
        ));
    }

    #[test]
    fn test_signed_ssv_message_creation_signature_too_long() {
        let message_id = MessageID::new([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);

        let mut signatures = vec![vec![0u8; 256]];
        signatures.push(vec![1u8; 257]); // Exceeds max length

        let operator_ids = vec![1, 2];
        let full_data = vec![];

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(SSVMessageError::SignatureTooLong {
                index: 1,
                length: 257,
                max: 256
            })
        ));
    }

    #[test]
    fn test_signed_ssv_message_creation_too_many_operator_ids() {
        let message_id = MessageID::new([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, message_id, vec![]);

        let signatures = vec![vec![0u8; 256]; 5];
        let operator_ids = vec![1u64; 14]; // Exceeds max of 13
        let full_data = vec![];

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(SSVMessageError::TooManyOperatorIDs {
                provided: 14,
                max: 13
            })
        ));
    }

    #[test]
    fn test_signed_ssv_message_creation_full_data_too_long() {
        let message_id = MessageID::new([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);

        let signatures = vec![vec![0u8; 256]];
        let operator_ids = vec![1];
        let full_data = vec![0u8; 4_194_533]; // Exceeds max

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(SSVMessageError::FullDataTooLong {
                length: 4_194_533,
                max: 4_194_532
            })
        ));
    }

    #[test]
    fn test_signed_ssv_message_encode_decode() {
        let message_id = MessageID::new([9u8; 56]);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            message_id.clone(),
            vec![100, 101, 102],
        );

        let signatures = vec![vec![10u8; 256], vec![20u8; 100]];
        let operator_ids = vec![1, 2];
        let full_data = vec![200u8; 1024];

        let signed_msg = SignedSSVMessage::new(
            signatures.clone(),
            operator_ids.clone(),
            ssv_msg.clone(),
            full_data.clone(),
        )
        .unwrap();

        let encoded = signed_msg.as_ssz_bytes();
        let decoded = SignedSSVMessage::from_ssz_bytes(&encoded).unwrap();

        assert_eq!(decoded, signed_msg);
    }

    #[test]
    fn test_ssvmessage_encode_decode_empty_data() {
        let message_id = MessageID::new([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id.clone(), vec![]);

        let encoded = ssv_msg.as_ssz_bytes();
        let decoded = SSVMessage::from_ssz_bytes(&encoded).unwrap();

        assert_eq!(decoded, ssv_msg);
    }

    #[test]
    fn test_ssvmessage_decode_invalid_length() {
        let bytes = vec![0u8; 56 + 8 + 3 - 1]; // Missing one byte in data
        let result = SSVMessage::from_ssz_bytes(&bytes);
        assert!(result.is_err());
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

    #[test]
    fn test_full_data_max_length() {
        let full_data = vec![0u8; SignedSSVMessage::MAX_FULL_DATA_LENGTH];
        let message_id = MessageID::new([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);
        let signatures = vec![vec![0u8; 256]];
        let operator_ids = vec![1];

        let signed_msg =
            SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data.clone());

        assert!(signed_msg.is_ok());

        let signed_msg = signed_msg.unwrap();
        assert_eq!(signed_msg.full_data(), &full_data);
    }

    #[test]
    fn test_full_data_exceeds_max_length() {
        let full_data = vec![0u8; SignedSSVMessage::MAX_FULL_DATA_LENGTH + 1];
        let message_id = MessageID::new([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);
        let signatures = vec![vec![0u8; 256]];
        let operator_ids = vec![1];

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(SSVMessageError::FullDataTooLong { length: _, max: _ })
        ));
    }
}
