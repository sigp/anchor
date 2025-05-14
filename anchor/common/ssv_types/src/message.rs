use std::{
    collections::HashSet,
    fmt::{Debug, Formatter},
};

use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use thiserror::Error;

use crate::{
    OperatorId,
    message::{
        SSVMessageError::{EmptyData, SSVDataTooBig},
        SignedSSVMessageError::{
            DuplicatedSigner, FullDataTooLong, NoSignatures, NoSigners,
            SignersAndSignaturesWithDifferentLength, SignersNotSorted, TooManyOperatorIDs,
            TooManySignatures, WrongRSASignatureSize, ZeroSigner,
        },
    },
    msgid::MessageId,
};

const QBFT_MSG_TYPE_SIZE: usize = 8;
const HEIGHT_SIZE: usize = 8;
const ROUND_SIZE: usize = 8;
const MAX_NO_JUSTIFICATION_SIZE: usize = 3616;
const MAX1_JUSTIFICATION_SIZE: usize = 50624;
const IDENTIFIER_SIZE: usize = 56; // same as MessageId length
const ROOT_SIZE: usize = 32;
const MAX_SIGNATURES: usize = 13;

// For partial signatures
const PARTIAL_SIGNATURE_SIZE: usize = 96;
const OPERATOR_ID_SIZE: usize = 8;
const VALIDATOR_INDEX_SIZE: usize = 8;
const SLOT_SIZE: usize = 8;
const PARTIAL_SIG_MSG_TYPE_SIZE: usize = 8;
const MAX_PARTIAL_SIGNATURE_MESSAGES: usize = 1000;
const ENCODING_OVERHEAD_DIVISOR: usize = 20;

// For RSA-based SignedSSVMessage
pub const RSA_SIGNATURE_SIZE: usize = 256;

// Additional from the Go code
const MAX_FULL_DATA_SIZE: usize = 4_194_532; // from spectypes.SignedSSVMessage

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

/// Defines the types of messages with explicit discriminant values.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
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

/// Represents errors that can occur while handling an SSVMessage.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SSVMessageError {
    #[error("SSVMessage data is empty")]
    EmptyData,

    #[error("SSVMessage data too large: got {got}, max {max}")]
    SSVDataTooBig { got: usize, max: usize },

    #[error("Wrong domain: got {got}, expected {want}")]
    WrongDomain { got: String, want: String },

    #[error("Signer {got} not in committee: {want:?}")]
    SignerNotInCommittee { got: u64, want: Vec<u64> },
}

/// Represents a bare SSVMessage with a type, ID, and data.
#[derive(Encode, Decode, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct SSVMessage {
    msg_type: MsgType,
    msg_id: MessageId, // Fixed-size [u8; 56]
    data: Vec<u8>,     // Variable-length byte array
}

impl Debug for SSVMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SSVMessage")
            .field("msg_type", &self.msg_type)
            .field("msg_id", &self.msg_id)
            .field("data", &hex::encode(&self.data))
            .finish()
    }
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
    pub fn new(
        msg_type: MsgType,
        msg_id: MessageId,
        data: Vec<u8>,
    ) -> Result<Self, SSVMessageError> {
        let ssv_message = SSVMessage {
            msg_type,
            msg_id,
            data,
        };
        ssv_message.validate()?;
        Ok(ssv_message)
    }

    pub fn validate(&self) -> Result<(), SSVMessageError> {
        if self.data.is_empty() {
            return Err(EmptyData);
        }
        match self.msg_type {
            MsgType::SSVConsensusMsgType => {
                if self.data.len() > MAX_ENCODED_CONSENSUS_MSG_SIZE {
                    return Err(SSVDataTooBig {
                        got: self.data.len(),
                        max: MAX_ENCODED_CONSENSUS_MSG_SIZE,
                    });
                }
            }
            MsgType::SSVPartialSignatureMsgType => {
                if self.data.len() > MAX_ENCODED_PARTIAL_SIGNATURE_SIZE {
                    return Err(SSVDataTooBig {
                        got: self.data.len(),
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
}

/// Errors that can occur while creating a `SignedSSVMessage`.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SignedSSVMessageError {
    #[error("Too many signatures: provided {provided}, maximum allowed is {max}.")]
    TooManySignatures { provided: usize, max: usize },

    #[error(
        "RSA Signature at index {index} has wrong size: {length} bytes, expected is {sig_length} bytes."
    )]
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

/// Represents a signed SSV Message with signatures, operator IDs, the message itself, and full
/// data.
#[derive(Encode, Decode, Clone, PartialEq, Eq)]
pub struct SignedSSVMessage {
    signatures: Vec<Vec<u8>>, // Vec of Vec<u8>, max 13 elements, each with 256 bytes
    operator_ids: Vec<OperatorId>, // Vec of OperatorID (u64), max 13 elements
    ssv_message: SSVMessage,  // SSVMessage: Required field
    full_data: Vec<u8>,       // Variable-length byte array, max 4,194,532 bytes
}

#[cfg(feature = "arbitrary-fuzz")]
use arbitrary::{Arbitrary, Result, Unstructured};

#[cfg(feature = "arbitrary-fuzz")]
use crate::consensus::{BeaconVote, QbftMessage};

#[cfg(feature = "arbitrary-fuzz")]
impl<'a> Arbitrary<'a> for SignedSSVMessage {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        // Generate arbitrary BeaconVote
        let beacon_vote = BeaconVote::arbitrary(u)?;

        // Generate arbitrary QbftMessage
        let qbft_message = QbftMessage::arbitrary(u)?;

        // Create arbitrary basic fields
        let signatures = Vec::<Vec<u8>>::arbitrary(u)?;
        let operator_ids = Vec::<OperatorId>::arbitrary(u)?;

        // Create SSV message with serialized QbftMessage
        let ssv_message = SSVMessage {
            msg_type: MsgType::arbitrary(u)?,
            msg_id: MessageId::arbitrary(u)?,
            data: qbft_message.as_ssz_bytes(), // Serialize QbftMessage to bytes
        };

        // Create the SignedSSVMessage with serialized BeaconVote
        Ok(SignedSSVMessage {
            signatures,
            operator_ids,
            ssv_message,
            full_data: beacon_vote.as_ssz_bytes(), // Serialize BeaconVote to bytes
        })
    }
}

impl Debug for SignedSSVMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let signatures = self.signatures.iter().map(hex::encode).collect::<Vec<_>>();

        f.debug_struct("SignedSSVMessage")
            .field("signatures", &signatures)
            .field("operator_ids", &self.operator_ids)
            .field("ssv_message", &self.ssv_message)
            .field("full_data", &hex::encode(&self.full_data))
            .finish()
    }
}

impl SignedSSVMessage {
    /// Creates a new `SignedSSVMessage` after validating constraints.
    ///
    /// # Arguments
    ///
    /// * `signatures` - A vector of signatures, each with [`RSA_SIGNATURE_SIZE`] bytes.
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
    /// use ssv_types::{
    ///     OperatorId,
    ///     message::{MessageId, MsgType, SSVMessage, SignedSSVMessage},
    /// };
    /// let ssv_msg = SSVMessage::new(
    ///     MsgType::SSVConsensusMsgType,
    ///     MessageId::from([0u8; 56]),
    ///     vec![1, 2, 3],
    /// )
    /// .unwrap();
    /// let signed_msg = SignedSSVMessage::new(
    ///     vec![vec![0; 256]],
    ///     vec![OperatorId(1)],
    ///     ssv_msg,
    ///     vec![4, 5, 6],
    /// )
    /// .unwrap();
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

    pub fn set_full_data(&mut self, data: Vec<u8>) {
        self.full_data = data;
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
        if self.signatures.len() > MAX_SIGNATURES {
            return Err(TooManySignatures {
                provided: self.signatures.len(),
                max: MAX_SIGNATURES,
            });
        }

        for (i, sig) in self.signatures.iter().enumerate() {
            if sig.len() != RSA_SIGNATURE_SIZE {
                return Err(WrongRSASignatureSize {
                    index: i,
                    length: sig.len(),
                    sig_length: RSA_SIGNATURE_SIZE,
                });
            }
        }

        if self.operator_ids.len() > MAX_SIGNATURES {
            return Err(TooManyOperatorIDs {
                provided: self.operator_ids.len(),
                max: MAX_SIGNATURES,
            });
        }

        if self.full_data.len() > MAX_FULL_DATA_SIZE {
            return Err(FullDataTooLong {
                length: self.full_data.len(),
                max: MAX_FULL_DATA_SIZE,
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
        // This check assumes that signers is sorted, so this rule should be after the check for
        // ErrSignersNotSorted.
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

#[cfg(test)]
mod tests {
    use std::iter;

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

    /// Returns a valid signature of exactly [`RSA_SIGNATURE_SIZE`] bytes.
    fn valid_signature() -> Vec<u8> {
        vec![0u8; RSA_SIGNATURE_SIZE]
    }

    /// Creates a valid, non-empty SSVMessage (ensuring it doesn’t exceed the max size).
    fn valid_ssv_message() -> SSVMessage {
        SSVMessage::new(MsgType::SSVConsensusMsgType, default_msg_id(), small_data())
            .expect("Creating a valid SSVMessage must succeed")
    }

    /// Creates a single-signer, single-signature valid SignedSSVMessage.
    fn valid_signed_ssv_message() -> SignedSSVMessage {
        let msg = valid_ssv_message();
        SignedSSVMessage::new(
            vec![valid_signature()],
            vec![OperatorId(1)],
            msg,
            vec![0xAB, 0xCD], // "full_data" well under max
        )
        .expect("Creating a valid SignedSSVMessage must succeed")
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
        let result = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            default_msg_id(),
            vec![],
        );

        match result {
            Err(SSVMessageError::EmptyData) => (), // success
            other => panic!("Expected EmptyData, got {:?}", other),
        }
    }

    /// Checks that data exceeding `MAX_CONSENSUS_MSG_SIZE` triggers `SSVDataTooBig`.
    #[test]
    fn test_consensus_message_too_big() {
        let oversized = vec![0u8; MAX_ENCODED_CONSENSUS_MSG_SIZE + 1];

        let result = SSVMessage::new(MsgType::SSVConsensusMsgType, default_msg_id(), oversized);

        match result {
            Err(SSVDataTooBig { got, max }) => {
                assert_eq!(got, MAX_ENCODED_CONSENSUS_MSG_SIZE + 1);
                assert_eq!(max, MAX_ENCODED_CONSENSUS_MSG_SIZE);
            }
            other => panic!("Expected SSVDataTooBig, got {:?}", other),
        }
    }

    /// Checks that data exceeding `MAX_PARTIAL_SIGNATURE_MSGS_SIZE` triggers `SSVDataTooBig`.
    #[test]
    fn test_partial_signature_message_too_big() {
        let oversized = vec![0u8; MAX_ENCODED_PARTIAL_SIGNATURE_SIZE + 1];

        let result = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            default_msg_id(),
            oversized,
        );

        match result {
            Err(SSVDataTooBig { got, max }) => {
                assert_eq!(got, MAX_ENCODED_PARTIAL_SIGNATURE_SIZE + 1);
                assert_eq!(max, MAX_ENCODED_PARTIAL_SIGNATURE_SIZE);
            }
            other => panic!("Expected SSVDataTooBig, got {:?}", other),
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

    // Tests for SignedSSVMessage
    //

    /// Checks that a valid single-signer message is created successfully.
    #[test]
    fn test_signed_ssv_message_valid() {
        let signed = valid_signed_ssv_message();

        assert_eq!(
            signed.operator_ids().len(),
            1,
            "Should have exactly one operator"
        );
        assert_eq!(
            signed.signatures().len(),
            1,
            "Should have exactly one signature"
        );
    }

    /// Checks that having more signatures than allowed triggers `TooManySignatures`.
    #[test]
    fn test_signed_ssv_message_too_many_signatures() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature(); MAX_SIGNATURES + 1];
        let ops = vec![OperatorId(1); MAX_SIGNATURES];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(TooManySignatures { provided, max }) => {
                assert_eq!(provided, MAX_SIGNATURES + 1);
                assert_eq!(max, MAX_SIGNATURES);
            }
            other => panic!("Expected TooManySignatures, got {:?}", other),
        }
    }

    /// Checks that a signature with the wrong size triggers `WrongRSASignatureSize`.
    #[test]
    fn test_signed_ssv_message_wrong_signature_size() {
        let ssv_msg = valid_ssv_message();
        let good = valid_signature();
        let mut bad = valid_signature();
        bad.pop(); // now it’s 255 bytes
        let sigs = vec![good, bad];
        let ops = vec![OperatorId(1), OperatorId(2)];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(WrongRSASignatureSize {
                index,
                length,
                sig_length,
            }) => {
                assert_eq!(index, 1);
                assert_eq!(length, 255);
                assert_eq!(sig_length, RSA_SIGNATURE_SIZE);
            }
            other => panic!("Expected WrongRSASignatureSize, got {:?}", other),
        }
    }

    /// Checks that having too many operator IDs triggers `TooManyOperatorIDs`.
    #[test]
    fn test_signed_ssv_message_too_many_operator_ids() {
        let ssv_msg = valid_ssv_message();
        let ops = vec![OperatorId(42); MAX_SIGNATURES + 1];
        let sigs = vec![valid_signature(); 2];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(TooManyOperatorIDs { provided, max }) => {
                assert_eq!(provided, MAX_SIGNATURES + 1);
                assert_eq!(max, MAX_SIGNATURES);
            }
            other => panic!("Expected TooManyOperatorIDs, got {:?}", other),
        }
    }

    /// Checks that having exactly MAX_SIGNATURES operator IDs doesn't triggers
    /// `TooManyOperatorIDs`.
    #[test]
    fn test_signed_ssv_message_max_operator_ids() {
        let ssv_msg = valid_ssv_message();
        // create MAX_SIGNATURES distinct operator IDs
        let ops = (1..=MAX_SIGNATURES)
            .map(|id| OperatorId(id as u64))
            .collect();
        let sigs = vec![valid_signature(); MAX_SIGNATURES];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Ok(_) => (),
            other => panic!("Expected Ok(_), got {:?}", other),
        }
    }

    /// Checks that `full_data` exceeding the limit triggers `FullDataTooLong`.
    #[test]
    fn test_signed_ssv_message_full_data_too_long() {
        let ssv_msg = valid_ssv_message();
        let huge_data = vec![0xAA; MAX_FULL_DATA_SIZE + 1];
        let sigs = vec![valid_signature()];
        let ops = vec![OperatorId(1)];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, huge_data);

        match result {
            Err(FullDataTooLong { length, max }) => {
                assert_eq!(length, MAX_FULL_DATA_SIZE + 1);
                assert_eq!(max, MAX_FULL_DATA_SIZE);
            }
            other => panic!("Expected FullDataTooLong, got {:?}", other),
        }
    }

    #[test]
    fn test_signed_ssv_message_full_data_max_length() {
        let ssv_msg = valid_ssv_message();
        let full_data = vec![0u8; MAX_FULL_DATA_SIZE];
        let sigs = vec![valid_signature()];
        let operator_ids = vec![OperatorId(1)];

        let signed_msg = SignedSSVMessage::new(sigs, operator_ids, ssv_msg, full_data.clone());

        match signed_msg {
            Ok(msg) => assert_eq!(msg.full_data(), &full_data),
            other => panic!("Expected SignedSSVMessage, got {:?}", other),
        }
    }

    /// Checks that providing zero operator IDs triggers `NoSigners`.
    #[test]
    fn test_signed_ssv_message_no_signers() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature()];
        let ops = vec![];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(NoSigners) => (),
            other => panic!("Expected NoSigners, got {:?}", other),
        }
    }

    /// Checks that providing zero signatures triggers `NoSignatures`.
    #[test]
    fn test_signed_ssv_message_no_signatures() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![];
        let ops = vec![OperatorId(1)];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(NoSignatures) => (),
            other => panic!("Expected NoSignatures, got {:?}", other),
        }
    }

    /// Checks that unsorted operator IDs triggers `SignersNotSorted`.
    #[test]
    fn test_signed_ssv_message_signers_not_sorted() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature(), valid_signature()];
        // Not sorted
        let ops = vec![OperatorId(10), OperatorId(2)];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignersNotSorted) => (),
            other => panic!("Expected SignersNotSorted, got {:?}", other),
        }
    }

    /// Checks that operator ID = 0 triggers `ZeroSigner`.
    #[test]
    fn test_signed_ssv_message_zero_signer() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature()];
        let ops = vec![OperatorId(0)];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(ZeroSigner) => (),
            other => panic!("Expected ZeroSigner, got {:?}", other),
        }
    }

    /// Checks that duplicate operator IDs triggers `DuplicatedSigner`.
    #[test]
    fn test_signed_ssv_message_duplicated_signer() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature(), valid_signature()];
        // Must be sorted to get past the sorting check
        let ops = vec![OperatorId(2), OperatorId(2)];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(DuplicatedSigner) => (),
            other => panic!("Expected DuplicatedSigner, got {:?}", other),
        }
    }

    /// Checks that signers != signatures triggers `SignersAndSignaturesWithDifferentLength`.
    #[test]
    fn test_signed_ssv_message_signer_sig_length_mismatch() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature(), valid_signature()];
        let ops = vec![OperatorId(1)];

        let result = SignedSSVMessage::new(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignersAndSignaturesWithDifferentLength) => (),
            other => panic!(
                "Expected SignersAndSignaturesWithDifferentLength, got {:?}",
                other
            ),
        }
    }

    /// Test encoding/decoding a valid SignedSSVMessage.
    #[test]
    fn test_signed_ssv_message_encode_decode() {
        let original = valid_signed_ssv_message();
        let bytes = original.as_ssz_bytes();

        let decoded = SignedSSVMessage::from_ssz_bytes(&bytes);

        assert!(
            decoded.is_ok(),
            "Decoding SignedSSVMessage failed: {:?}",
            decoded.err()
        );
        let decoded = decoded.expect("Should decode successfully");
        assert_eq!(
            decoded, original,
            "Decoded SignedSSVMessage differs from original"
        );
    }

    /// If we pass an invalid `SSVMessage` (e.g. empty data) to SignedSSVMessage,
    /// we expect a `SignedSSVMessageError::SSVMessagError(SSVMessageError::EmptyData)`.
    #[test]
    fn test_invalid_ssv_message_propagates_error() {
        let empty_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, default_msg_id(), vec![]);
        // Should fail to create the SSVMessage, but let's check the code path
        // if we forcibly pass this "erroneous" SSVMessage.
        assert!(
            empty_msg.is_err(),
            "Constructing an empty-data SSVMessage must fail"
        );

        // Force the scenario: pretend we got an SSVMessage from somewhere else
        // that didn't call `new()`, and attempt to use it:
        let forcibly_invalid_msg = SSVMessage {
            msg_type: MsgType::SSVConsensusMsgType,
            msg_id: default_msg_id(),
            data: vec![], // still empty
        };
        let result = SignedSSVMessage::new(
            vec![valid_signature()],
            vec![OperatorId(1)],
            forcibly_invalid_msg,
            vec![],
        );

        match result {
            Err(SignedSSVMessageError::SSVMessagError(SSVMessageError::EmptyData)) => (),
            other => panic!("Expected SSVMessagError(EmptyData), got {:?}", other),
        }
    }

    // Tests for aggregator logic
    //

    /// Checks that aggregator merges signers/signatures and sorts them by operator ID.
    #[test]
    fn test_signed_ssv_message_aggregation() {
        let mut base = valid_signed_ssv_message(); // has operator_ids = [1]
        let extra = SignedSSVMessage::new(
            vec![valid_signature()],
            vec![OperatorId(5)],
            valid_ssv_message(),
            vec![0xEE],
        )
        .expect("Should be valid");

        base.aggregate(iter::once(extra));
        let ops = base.operator_ids();
        let sigs = base.signatures();
        assert_eq!(
            ops,
            &[OperatorId(1), OperatorId(5)],
            "Expected sorted [1,5]"
        );
        assert_eq!(sigs.len(), 2, "Expected 2 signatures total");
    }
}
