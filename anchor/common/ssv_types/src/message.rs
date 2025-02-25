use crate::message::SSVMessageError::{EmptyData, SSVDataTooBig};
use crate::message::SignedSSVMessageError::{
    DuplicatedSigner, FullDataTooLong, NoSignatures, NoSigners,
    SignersAndSignaturesWithDifferentLength, SignersNotSorted, TooManyOperatorIDs,
    TooManySignatures, WrongRSASignatureSize, ZeroSigner,
};
use crate::msgid::MessageId;
use crate::OperatorId;
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use std::collections::HashSet;
use std::fmt::Debug;
use thiserror::Error;

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
        let value =
            u64::from_le_bytes(bytes.try_into().map_err(|_| {
                DecodeError::BytesInvalid(format!("Invalid length: {}", bytes.len()))
            })?);
        value.try_into()
    }
}

/// Represents an SSV Message with type, ID, and data.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq)]
pub struct SSVMessage {
    msg_type: MsgType,
    msg_id: MessageId, // Fixed-size [u8; 56]
    data: Vec<u8>,     // Variable-length byte array
}

impl SSVMessage {
    /// Creates a new `SSVMessage`.
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
    /// use ssv_types::message::{MessageId, MsgType, SSVMessage};
    /// let message_id = MessageId::from([0u8; 56]);
    /// let msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![1, 2, 3]);
    /// ```
    pub fn new(msg_type: MsgType, msg_id: MessageId, data: Vec<u8>) -> Self {
        SSVMessage {
            msg_type,
            msg_id,
            data,
        }
    }

    pub fn validate(&self) -> Result<(), SSVMessageError> {
        if self.data.is_empty() {
            return Err(EmptyData);
        }

        if self.data.len() > SignedSSVMessage::MAX_FULL_DATA_LENGTH {
            return Err(SSVDataTooBig {
                got: self.data.len(),
                max: SignedSSVMessage::MAX_FULL_DATA_LENGTH,
            });
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
}

/// Represents a signed SSV Message with signatures, operator IDs, the message itself, and full data.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq)]
pub struct SignedSSVMessage {
    signatures: Vec<Vec<u8>>, // Vec of Vec<u8>, max 13 elements, each with 256 bytes
    operator_ids: Vec<OperatorId>, // Vec of OperatorID (u64), max 13 elements
    ssv_message: SSVMessage,  // SSVMessage: Required field
    full_data: Vec<u8>,       // Variable-length byte array, max 4,194,532 bytes
}

impl SignedSSVMessage {
    /// Maximum allowed number of signatures and operator IDs.
    pub const MAX_SIGNATURES: usize = 13;
    /// Length for each signature in bytes.
    pub const SIGNATURE_LENGTH: usize = 256;
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
    /// use ssv_types::message::{MessageId, MsgType, SSVMessage, SignedSSVMessage};
    /// use ssv_types::OperatorId;
    /// let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, MessageId::from([0u8; 56]), vec![1,2,3]);
    /// let signed_msg = SignedSSVMessage::new(vec![vec![0; 256]], vec![OperatorId(1)], ssv_msg, vec![4,5,6]).unwrap();
    /// ```
    pub fn new(
        signatures: Vec<Vec<u8>>,
        operator_ids: Vec<OperatorId>,
        ssv_message: SSVMessage,
        full_data: Vec<u8>,
    ) -> Result<Self, SignedSSVMessageError> {
        let signed_ssv_message = SignedSSVMessage {
            signatures,
            operator_ids,
            ssv_message,
            full_data,
        };

        signed_ssv_message.validate()?;

        Ok(signed_ssv_message)
    }

    /// Returns a reference to the signatures.
    pub fn signatures(&self) -> &Vec<Vec<u8>> {
        &self.signatures
    }

    /// Returns a reference to the operator IDs.
    pub fn operator_ids(&self) -> &Vec<OperatorId> {
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

    /// Aggregate a set of signed ssv messages into Self
    pub fn aggregate<I>(&mut self, others: I)
    where
        I: IntoIterator<Item = SignedSSVMessage>,
    {
        for signed_msg in others {
            // These will only all have 1 signature/operator, but we call extend for safety
            self.signatures.extend(signed_msg.signatures);
            self.operator_ids.extend(signed_msg.operator_ids);
        }

        // Maintain id <-> sig pairing during sorting
        let mut sig_pairs: Vec<_> = self
            .signatures
            .iter()
            .cloned()
            .zip(self.operator_ids.iter())
            .collect();

        sig_pairs.sort_by_key(|&(_, op_id)| *op_id);

        let (sorted_signatures, sorted_operator_ids) = sig_pairs.into_iter().unzip();
        self.signatures = sorted_signatures;
        self.operator_ids = sorted_operator_ids;
    }

    // Validate the signed message to ensure that it is well formed for qbft processing
    pub fn validate(&self) -> Result<(), SignedSSVMessageError> {
        if self.signatures.len() > SignedSSVMessage::MAX_SIGNATURES {
            return Err(TooManySignatures {
                provided: self.signatures.len(),
                max: SignedSSVMessage::MAX_SIGNATURES,
            });
        }

        for (i, sig) in self.signatures.iter().enumerate() {
            if sig.len() != SignedSSVMessage::SIGNATURE_LENGTH {
                return Err(WrongRSASignatureSize {
                    index: i,
                    length: sig.len(),
                    sig_length: SignedSSVMessage::SIGNATURE_LENGTH,
                });
            }
        }

        if self.operator_ids.len() > SignedSSVMessage::MAX_SIGNATURES {
            return Err(TooManyOperatorIDs {
                provided: self.operator_ids.len(),
                max: SignedSSVMessage::MAX_SIGNATURES,
            });
        }

        if self.full_data.len() > SignedSSVMessage::MAX_FULL_DATA_LENGTH {
            return Err(FullDataTooLong {
                length: self.full_data.len(),
                max: SignedSSVMessage::MAX_FULL_DATA_LENGTH,
            });
        }

        // Rule: Must have at least one signer
        if self.operator_ids.is_empty() {
            return Err(NoSigners);
        }

        if self.signatures.is_empty() {
            return Err(NoSignatures);
        }

        if !self.operator_ids.is_sorted() {
            return Err(SignersNotSorted);
        }

        // Note: Len Signers & Operators will only be > 1 after commit aggregation

        // Rule: Signer can't be zero
        if self.operator_ids.iter().any(|&id| *id == 0) {
            return Err(ZeroSigner);
        }

        // Rule: Signers must be unique
        // This check assumes that signers is sorted, so this rule should be after the check for ErrSignersNotSorted.
        let mut seen_ids = HashSet::with_capacity(self.operator_ids.len());
        for &id in &self.operator_ids {
            if !seen_ids.insert(id) {
                return Err(DuplicatedSigner);
            }
        }

        // Rule: Len(Signers) must be equal to Len(Signatures)
        if self.operator_ids.len() != self.signatures.len() {
            return Err(SignersAndSignaturesWithDifferentLength);
        }

        self.ssv_message.validate()?;

        Ok(())
    }
}

/// Represents errors that can occur while creating a `SignedSSVMessage`.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SignedSSVMessageError {
    #[error("Too many signatures: provided {provided}, maximum allowed is {max}.")]
    TooManySignatures { provided: usize, max: usize },

    #[error("RSA Signature at index {index} has wrong size: {length} bytes, expected is {sig_length} bytes.")]
    WrongRSASignatureSize {
        index: usize,
        length: usize,
        sig_length: usize,
    },

    #[error("Too many operator IDs: provided {provided}, maximum allowed is {max}.")]
    TooManyOperatorIDs { provided: usize, max: usize },

    #[error("Full data is too long: {length} bytes, maximum allowed is {max} bytes.")]
    FullDataTooLong { length: usize, max: usize },

    #[error("No signers were provided (must have at least one signer).")]
    NoSigners,

    #[error("Signers and signatures must have the same length.")]
    SignersAndSignaturesWithDifferentLength,

    #[error("At least one signer has ID = 0, which is invalid.")]
    ZeroSigner,

    #[error("Signers are not sorted by their IDs.")]
    SignersNotSorted,

    #[error("No signatures provided.")]
    NoSignatures,

    #[error("A duplicated signer was found (all signers must be unique).")]
    DuplicatedSigner,

    #[error("Invalid SSVMessage: {0}")]
    SSVMessagError(#[from] SSVMessageError),
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SSVMessageError {
    #[error("SSVMessage data is empty")]
    EmptyData,

    #[error("SSVMessage data too large: got {got}, max {max}")]
    SSVDataTooBig { got: usize, max: usize },

    #[error("Event message is not supported in this context")]
    EventMessage,

    #[error("Unknown SSV message type: {got}")]
    UnknownSSVMessageType { got: u8 },

    #[error("Wrong domain: got {got}, expected {want}")]
    WrongDomain { got: String, want: String },

    #[error("Invalid role: {role}")]
    InvalidRole { role: u8 },

    #[error("Signer {got} not in committee: {want:?}")]
    SignerNotInCommittee { got: u64, want: Vec<u64> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssz::{Decode, Encode};

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
        let message_id = MessageId::from([7u8; 56]);
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
        let message_id = MessageId::from([0u8; 56]);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            message_id,
            vec![1, 2, 3],
        );

        let signatures = vec![vec![0u8; 256], vec![1u8; 256]];
        let operator_ids = vec![OperatorId(1), OperatorId(2)];
        let full_data = vec![255u8; 4_194_532];

        let signed_msg = SignedSSVMessage::new(
            signatures.clone(),
            operator_ids.clone(),
            ssv_msg.clone(),
            full_data.clone(),
        );

        assert!(signed_msg.is_ok());

        let signed_msg = signed_msg.unwrap();
        assert_eq!(*signed_msg.signatures(), signatures);
        assert_eq!(**signed_msg.operator_ids(), operator_ids);
        assert_eq!(signed_msg.ssv_message(), &ssv_msg);
        assert_eq!(signed_msg.full_data(), &full_data);
    }

    #[test]
    fn test_signed_ssv_message_creation_too_many_signatures() {
        let message_id = MessageId::from([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);

        let signatures = vec![vec![0u8; 256]; 14]; // Exceeds max of 13
        let operator_ids = vec![OperatorId(1); 13];
        let full_data = vec![];

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(TooManySignatures {
                provided: 14,
                max: 13
            })
        ));
    }

    #[test]
    fn test_signed_ssv_message_creation_signature_too_long() {
        let message_id = MessageId::from([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);

        let mut signatures = vec![vec![0u8; 256]];
        signatures.push(vec![1u8; 257]); // Exceeds max length

        let operator_ids = vec![OperatorId(1), OperatorId(2)];
        let full_data = vec![];

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(WrongRSASignatureSize {
                index: 1,
                length: 257,
                sig_length: SignedSSVMessage::SIGNATURE_LENGTH,
            })
        ));
    }

    #[test]
    fn test_signed_ssv_message_creation_too_many_operator_ids() {
        let message_id = MessageId::from([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, message_id, vec![]);

        let signatures = vec![vec![0u8; 256]; 5];
        let operator_ids = vec![OperatorId(1); 14]; // Exceeds max of 13
        let full_data = vec![];

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(TooManyOperatorIDs {
                provided: 14,
                max: 13
            })
        ));
    }

    #[test]
    fn test_signed_ssv_message_creation_full_data_too_long() {
        let message_id = MessageId::from([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);

        let signatures = vec![vec![0u8; 256]];
        let operator_ids = vec![OperatorId(1)];
        let full_data = vec![0u8; 4_194_533]; // Exceeds max

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(FullDataTooLong {
                length: 4_194_533,
                max: 4_194_532
            })
        ));
    }

    #[test]
    fn test_signed_ssv_message_encode_decode() {
        let message_id = MessageId::from([9u8; 56]);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            message_id.clone(),
            vec![100, 101, 102],
        );

        let signatures = vec![vec![10u8; 256], vec![20u8; 256]];
        let operator_ids = vec![OperatorId(1), OperatorId(2)];
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
        let message_id = MessageId::from([0u8; 56]);
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
        let message_id = MessageId::from([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![0u8, 1]);
        let signatures = vec![vec![0u8; 256]];
        let operator_ids = vec![OperatorId(1)];

        let signed_msg =
            SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data.clone());

        assert!(
            signed_msg.is_ok(),
            "Error creating SignedSSVMessage: {:?}",
            signed_msg.err()
        );

        let signed_msg = signed_msg.unwrap();
        assert_eq!(signed_msg.full_data(), &full_data);
    }

    #[test]
    fn test_full_data_exceeds_max_length() {
        let full_data = vec![0u8; SignedSSVMessage::MAX_FULL_DATA_LENGTH + 1];
        let message_id = MessageId::from([0u8; 56]);
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, vec![]);
        let signatures = vec![vec![0u8; 256]];
        let operator_ids = vec![OperatorId(1)];

        let signed_msg = SignedSSVMessage::new(signatures, operator_ids, ssv_msg, full_data);

        assert!(matches!(
            signed_msg,
            Err(SignedSSVMessageError::FullDataTooLong { length: _, max: _ })
        ));
    }
}
