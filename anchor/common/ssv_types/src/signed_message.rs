use std::{
    collections::HashSet,
    fmt::{Debug, Display, Formatter},
};

use base64::prelude::*;
use serde::{Deserialize, de::Error};
use serde_json::Value;
use ssz_derive::{Decode, Encode};
use ssz_types::VariableList;
use thiserror::Error;
use tree_hash_derive::TreeHash;
use types::typenum::{Prod, Sum, U8, U13, U388, U836, U1000, U1000000};

use crate::{
    MAX_SIGNATURES, OperatorId, RSA_SIGNATURE_SIZE,
    message::{SSVMessage, SSVMessageError},
};

/// SignedSSVMessage.FullData max size: 8388836 (from Go spec)
/// 8388836 = 8000000 + 388836 = 8 * 1000000 + 388836
/// We need to construct 388836 = 388 * 1000 + 836 = 388000 + 836
type SSVMessageFullDataLen = Sum<Prod<U8, U1000000>, Sum<Prod<U388, U1000>, U836>>;
/// Errors that can occur while creating a `SignedSSVMessage`.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SignedSSVMessageError {
    #[error("Too many signatures: provided {provided}, maximum allowed is {max}.")]
    TooManySignatures { provided: usize, max: usize },

    #[error("Too many operator IDs: provided {provided}, maximum allowed is {max}.")]
    TooManyOperatorIDs { provided: usize, max: usize },

    #[error("Full data is too long: {provided} bytes, maximum allowed is {max} bytes.")]
    FullDataTooLong { provided: usize, max: usize },

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
    SSVMessageError(#[from] SSVMessageError),
}

/// Maximum of 13 signatures.
pub type SignatureList = VariableList<[u8; 256], U13>;

/// Represents a signed SSV Message with signatures, operator IDs, the message itself, and full
/// data.
#[derive(Encode, Decode, Clone, PartialEq, Eq, Deserialize, TreeHash)]
pub struct SignedSSVMessage {
    #[serde(rename = "Signatures")]
    #[serde(deserialize_with = "deserialize_base64_signatures")]
    signatures: SignatureList,

    #[serde(rename = "OperatorIDs")]
    operator_ids: VariableList<OperatorId, U13>,

    #[serde(rename = "SSVMessage")]
    ssv_message: SSVMessage,

    #[serde(rename = "FullData")]
    #[serde(deserialize_with = "deserialize_base64_or_empty")]
    full_data: VariableList<u8, SSVMessageFullDataLen>,
}

impl SignedSSVMessage {
    pub fn new(
        signatures: SignatureList,
        operator_ids: VariableList<OperatorId, U13>,
        ssv_message: SSVMessage,
        full_data: VariableList<u8, SSVMessageFullDataLen>,
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
    ///     message::{MsgType, SSVMessage},
    ///     msgid::MessageId,
    ///     signed_message::SignedSSVMessage,
    /// };
    /// let ssv_msg = SSVMessage::new_from_vec(
    ///     MsgType::SSVConsensusMsgType,
    ///     MessageId::from([0u8; 56]),
    ///     vec![1, 2, 3],
    /// )
    /// .unwrap();
    /// let signed_msg = SignedSSVMessage::new_from_vecs(
    ///     vec![[0; 256]],
    ///     vec![OperatorId(1)],
    ///     ssv_msg,
    ///     vec![4, 5, 6],
    /// )
    /// .unwrap();
    /// ```
    pub fn new_from_vecs(
        signatures: Vec<[u8; RSA_SIGNATURE_SIZE]>,
        operator_ids: Vec<OperatorId>,
        ssv_message: SSVMessage,
        full_data: Vec<u8>,
    ) -> Result<Self, SignedSSVMessageError> {
        Self::new(
            crate::vec_to_variable_list!(signatures, SignedSSVMessageError::TooManySignatures)?,
            crate::vec_to_variable_list!(operator_ids, SignedSSVMessageError::TooManyOperatorIDs)?,
            ssv_message,
            crate::vec_to_variable_list!(full_data, SignedSSVMessageError::FullDataTooLong)?,
        )
    }

    /// Returns a reference to the signatures.
    pub fn signatures(&self) -> &SignatureList {
        &self.signatures
    }

    /// Returns a reference to the operator IDs.
    pub fn operator_ids(&self) -> &[OperatorId] {
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

    pub fn set_full_data(&mut self, data: Vec<u8>) -> Result<(), SignedSSVMessageError> {
        self.full_data =
            crate::vec_to_variable_list!(data, SignedSSVMessageError::FullDataTooLong)?;
        Ok(())
    }

    /// Returns a clone of this SignedSSVMessage with empty full_data.
    /// This matches the Go implementation's WithoutFullData() method used for justifications.
    pub fn without_full_data(&self) -> Self {
        let mut cloned = self.clone();
        cloned.full_data = VariableList::empty();
        cloned
    }

    /// Aggregate a set of signed ssv messages into Self
    pub fn aggregate<I>(&mut self, others: I) -> Result<(), SignedSSVMessageError>
    where
        I: IntoIterator<Item = SignedSSVMessage>,
    {
        for signed_msg in others {
            if signed_msg.operator_ids.len() != signed_msg.signatures.len() {
                return Err(SignedSSVMessageError::SignersAndSignaturesWithDifferentLength);
            }

            // These will only all have 1 signature/operator, but we call extend for safety
            for signature in signed_msg.signatures.into_iter() {
                self.signatures.push(signature).map_err(|_| {
                    SignedSSVMessageError::TooManySignatures {
                        provided: self.signatures.len() + 1,
                        max: MAX_SIGNATURES,
                    }
                })?;
            }
            for operator_id in signed_msg.operator_ids.into_iter() {
                self.operator_ids.push(operator_id).map_err(|_| {
                    SignedSSVMessageError::TooManyOperatorIDs {
                        provided: self.operator_ids.len() + 1,
                        max: MAX_SIGNATURES,
                    }
                })?;
            }
        }

        // Maintain id <-> sig pairing during sorting
        let mut sig_pairs: Vec<_> = self
            .signatures
            .iter()
            .cloned()
            .zip(self.operator_ids.iter())
            .collect();

        sig_pairs.sort_by_key(|&(_, op_id)| *op_id);

        let (sorted_signatures, sorted_operator_ids) = sig_pairs.iter().cloned().unzip();
        self.signatures = crate::vec_to_variable_list!(
            sorted_signatures,
            SignedSSVMessageError::TooManySignatures
        )?;
        self.operator_ids = crate::vec_to_variable_list!(
            sorted_operator_ids,
            SignedSSVMessageError::TooManyOperatorIDs
        )?;
        Ok(())
    }

    // Validate the signed message to ensure that it is well formed for qbft processing
    pub fn validate(&self) -> Result<(), SignedSSVMessageError> {
        // Rule: Must have at least one signer
        if self.operator_ids.is_empty() {
            return Err(SignedSSVMessageError::NoSigners);
        }

        if self.signatures.is_empty() {
            return Err(SignedSSVMessageError::NoSignatures);
        }

        if !self.operator_ids.is_sorted() {
            return Err(SignedSSVMessageError::SignersNotSorted);
        }

        // Note: Len Signers & Operators will only be > 1 after commit aggregation

        // Rule: Signer can't be zero
        if self.operator_ids.iter().any(|&id| *id == 0) {
            return Err(SignedSSVMessageError::ZeroSigner);
        }

        // Rule: Signers must be unique
        // This check assumes that signers is sorted, so this rule should be after the check for
        // ErrSignersNotSorted.
        let mut seen_ids = HashSet::with_capacity(self.operator_ids.len());
        for &id in &self.operator_ids {
            if !seen_ids.insert(id) {
                return Err(SignedSSVMessageError::DuplicatedSigner);
            }
        }

        // Rule: Len(Signers) must be equal to Len(Signatures)
        if self.operator_ids.len() != self.signatures.len() {
            return Err(SignedSSVMessageError::SignersAndSignaturesWithDifferentLength);
        }

        self.ssv_message.validate()?;

        Ok(())
    }
}

fn deserialize_base64_or_empty<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: TryFrom<Vec<u8>>,
{
    let value = Value::deserialize(deserializer)?;

    match value {
        Value::Null => Ok(Vec::new()), // Return empty Vec for null values
        Value::String(s) => BASE64_STANDARD
            .decode(s.as_bytes())
            .map_err(D::Error::custom),
        _ => Err(D::Error::custom("Expected null or a base64 string")),
    }
    .and_then(|vec| {
        vec.try_into()
            .map_err(|_| D::Error::custom("Failed to convert from Vec<u8> to actual type"))
    })
}

fn deserialize_base64_signatures<'de, D>(deserializer: D) -> Result<SignatureList, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string_vec: Vec<String> = serde::Deserialize::deserialize(deserializer)?;

    let mut signatures = VariableList::empty();

    for string in string_vec {
        let mut signature = [0u8; RSA_SIGNATURE_SIZE];
        let decoded_len = BASE64_STANDARD
            .decode_slice(string.as_bytes(), &mut signature)
            .map_err(serde::de::Error::custom)?;

        if decoded_len != RSA_SIGNATURE_SIZE {
            return Err(D::Error::custom("Incorrect size for signature"));
        }

        if let Err(err) = signatures.push(signature) {
            return Err(D::Error::custom(format!("Too many signatures: {err:?}")));
        }
    }

    Ok(signatures)
}

#[cfg(feature = "arbitrary-fuzz")]
mod arbitrary_impls {
    use arbitrary::{Arbitrary, Result, Unstructured};
    use ssz::Encode;

    use super::*;
    use crate::{
        consensus::{BeaconVote, QbftMessage},
        message::MsgType,
        msgid::MessageId,
    };

    impl<'a> Arbitrary<'a> for SignedSSVMessage {
        fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
            // Generate arbitrary BeaconVote
            let beacon_vote = BeaconVote::arbitrary(u)?;

            // Generate arbitrary QbftMessage
            let qbft_message = QbftMessage::arbitrary(u)?;

            // Create arbitrary basic fields
            let signatures = Vec::<[u8; RSA_SIGNATURE_SIZE]>::arbitrary(u)?;
            let operator_ids = Vec::<OperatorId>::arbitrary(u)?;

            // Create SSV message with serialized QbftMessage
            let ssv_message = SSVMessage::new_from_vec(
                MsgType::arbitrary(u)?,
                MessageId::arbitrary(u)?,
                qbft_message.as_ssz_bytes(), // Serialize QbftMessage to bytes
            )
            .expect("Valid SSVMessage");

            // Create the SignedSSVMessage with serialized BeaconVote
            Ok(SignedSSVMessage::new_from_vecs(
                signatures,
                operator_ids,
                ssv_message,
                beacon_vote.as_ssz_bytes(), // Serialize BeaconVote to bytes
            )
            .expect("Valid SignedSSVMessage"))
        }
    }
}

// This impl is meant for displaying messages in debug logs, where we usually do not need to know,
// e.g., the exact byte values of signatures. The `Debug` impl remains fully featured for tracing
// logs or other special cases.
impl Display for SignedSSVMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedSSVMessage")
            .field("signatures", &self.signatures.len())
            .field("operator_ids", &self.operator_ids)
            .field("ssv_message", &self.ssv_message)
            .field("full_data", &!self.full_data.is_empty())
            .finish()
    }
}

impl Debug for SignedSSVMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let signatures = (&self.signatures)
            .into_iter()
            .map(|v| v.to_vec())
            .map(hex::encode)
            .collect::<Vec<_>>();

        f.debug_struct("SignedSSVMessage")
            .field("signatures", &signatures)
            .field("operator_ids", &self.operator_ids)
            .field("ssv_message", &self.ssv_message)
            .field("full_data", &hex::encode(&*self.full_data))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use ssz::{Decode, Encode};
    use typenum::Unsigned;

    use super::*;
    use crate::{message::MsgType, test_utils::*};

    const MAX_FULL_DATA_SIZE: usize = SSVMessageFullDataLen::USIZE;

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

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignedSSVMessageError::TooManySignatures { provided, max }) => {
                assert_eq!(provided, MAX_SIGNATURES + 1);
                assert_eq!(max, MAX_SIGNATURES);
            }
            other => panic!("Expected TooManySignatures, got {other:?}"),
        }
    }

    /// Checks that having too many operator IDs triggers `TooManyOperatorIDs`.
    #[test]
    fn test_signed_ssv_message_too_many_operator_ids() {
        let ssv_msg = valid_ssv_message();
        let ops = vec![OperatorId(42); MAX_SIGNATURES + 1];
        let sigs = vec![valid_signature(); 2];

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignedSSVMessageError::TooManyOperatorIDs { provided, max }) => {
                assert_eq!(provided, MAX_SIGNATURES + 1);
                assert_eq!(max, MAX_SIGNATURES);
            }
            other => panic!("Expected TooManyOperatorIDs, got {other:?}"),
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

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Ok(_) => (),
            other => panic!("Expected Ok(_), got {other:?}"),
        }
    }

    /// Checks that `full_data` exceeding the limit triggers `FullDataTooLong`.
    #[test]
    fn test_signed_ssv_message_full_data_too_long() {
        let ssv_msg = valid_ssv_message();
        let huge_data = vec![0xAA; MAX_FULL_DATA_SIZE + 1];
        let sigs = vec![valid_signature()];
        let ops = vec![OperatorId(1)];

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, huge_data);

        match result {
            Err(SignedSSVMessageError::FullDataTooLong { provided, max }) => {
                assert_eq!(provided, MAX_FULL_DATA_SIZE + 1);
                assert_eq!(max, MAX_FULL_DATA_SIZE);
            }
            other => panic!("Expected FullDataTooLong, got {other:?}"),
        }
    }

    #[test]
    fn test_signed_ssv_message_full_data_max_length() {
        let ssv_msg = valid_ssv_message();
        let full_data = vec![0u8; MAX_FULL_DATA_SIZE];
        let sigs = vec![valid_signature()];
        let operator_ids = vec![OperatorId(1)];

        let signed_msg =
            SignedSSVMessage::new_from_vecs(sigs, operator_ids, ssv_msg, full_data.clone());

        match signed_msg {
            Ok(msg) => assert_eq!(msg.full_data(), &full_data),
            other => panic!("Expected SignedSSVMessage, got {other:?}"),
        }
    }

    /// Checks that providing zero operator IDs triggers `NoSigners`.
    #[test]
    fn test_signed_ssv_message_no_signers() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature()];
        let ops = vec![];

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignedSSVMessageError::NoSigners) => (),
            other => panic!("Expected NoSigners, got {other:?}"),
        }
    }

    /// Checks that providing zero signatures triggers `NoSignatures`.
    #[test]
    fn test_signed_ssv_message_no_signatures() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![];
        let ops = vec![OperatorId(1)];

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignedSSVMessageError::NoSignatures) => (),
            other => panic!("Expected NoSignatures, got {other:?}"),
        }
    }

    /// Checks that unsorted operator IDs triggers `SignersNotSorted`.
    #[test]
    fn test_signed_ssv_message_signers_not_sorted() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature(), valid_signature()];
        // Not sorted
        let ops = vec![OperatorId(10), OperatorId(2)];

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignedSSVMessageError::SignersNotSorted) => (),
            other => panic!("Expected SignersNotSorted, got {other:?}"),
        }
    }

    /// Checks that operator ID = 0 triggers `ZeroSigner`.
    #[test]
    fn test_signed_ssv_message_zero_signer() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature()];
        let ops = vec![OperatorId(0)];

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignedSSVMessageError::ZeroSigner) => (),
            other => panic!("Expected ZeroSigner, got {other:?}"),
        }
    }

    /// Checks that duplicate operator IDs triggers `DuplicatedSigner`.
    #[test]
    fn test_signed_ssv_message_duplicated_signer() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature(), valid_signature()];
        // Must be sorted to get past the sorting check
        let ops = vec![OperatorId(2), OperatorId(2)];

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignedSSVMessageError::DuplicatedSigner) => (),
            other => panic!("Expected DuplicatedSigner, got {other:?}"),
        }
    }

    /// Checks that signers != signatures triggers `SignersAndSignaturesWithDifferentLength`.
    #[test]
    fn test_signed_ssv_message_signer_sig_length_mismatch() {
        let ssv_msg = valid_ssv_message();
        let sigs = vec![valid_signature(), valid_signature()];
        let ops = vec![OperatorId(1)];

        let result = SignedSSVMessage::new_from_vecs(sigs, ops, ssv_msg, vec![]);

        match result {
            Err(SignedSSVMessageError::SignersAndSignaturesWithDifferentLength) => (),
            other => panic!("Expected SignersAndSignaturesWithDifferentLength, got {other:?}"),
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
        let empty_msg =
            SSVMessage::new_from_vec(MsgType::SSVConsensusMsgType, default_msg_id(), vec![]);
        // Should fail to create the SSVMessage, but let's check the code path
        // if we forcibly pass this "erroneous" SSVMessage.
        assert!(
            empty_msg.is_err(),
            "Constructing an empty-data SSVMessage must fail"
        );

        // Force the scenario: pretend we got an SSVMessage from somewhere else
        // that didn't call `new()`, and attempt to use it:
        let forcibly_invalid_msg = SSVMessage::new_unvalidated(
            MsgType::SSVConsensusMsgType,
            default_msg_id(),
            VariableList::empty(), // still empty
        );
        let result = SignedSSVMessage::new_from_vecs(
            vec![valid_signature()],
            vec![OperatorId(1)],
            forcibly_invalid_msg,
            vec![],
        );

        match result {
            Err(SignedSSVMessageError::SSVMessageError(SSVMessageError::EmptyData)) => (),
            other => panic!("Expected SSVMessagError(EmptyData), got {other:?}"),
        }
    }

    // Tests for aggregator logic
    //

    /// Checks that aggregator merges signers/signatures and sorts them by operator ID.
    #[test]
    fn test_signed_ssv_message_aggregation() {
        let mut base = valid_signed_ssv_message(); // has operator_ids = [1]
        let extra = SignedSSVMessage::new_from_vecs(
            vec![valid_signature()],
            vec![OperatorId(5)],
            valid_ssv_message(),
            vec![0xEE],
        )
        .expect("Should be valid");

        base.aggregate(iter::once(extra))
            .expect("Aggregation should succeed");
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
