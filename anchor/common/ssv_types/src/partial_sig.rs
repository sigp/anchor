use serde::Deserialize;
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use types::{
    Hash256, Signature, Slot, VariableList,
    typenum::{Sum, U512, U1000},
};

use crate::{OperatorId, ValidatorIndex, deserializers::*};

/// Maximum number of partial signature messages: 1512
/// Calculated as 1000 + 512 = 1512
pub type PartialSignatureMessagesLen = Sum<U1000, U512>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(from = "u64", into = "u64")]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub enum PartialSignatureKind {
    // PostConsensusPartialSig is a partial signature over a decided duty (attestation data,
    // block, etc)
    PostConsensus = 0,
    // RandaoPartialSig is a partial signature over randao reveal
    RandaoPartialSig = 1,
    // SelectionProofPartialSig is a partial signature for aggregator selection proof
    SelectionProofPartialSig = 2,
    // ContributionProofs is the partial selection proofs for sync committee contributions (it's
    // an array of sigs)
    ContributionProofs = 3,
    // ValidatorRegistrationPartialSig is a partial signature over a ValidatorRegistration object
    ValidatorRegistration = 4,
    // VoluntaryExitPartialSig is a partial signature over a VoluntaryExit object
    VoluntaryExit = 5,
}

impl From<u64> for PartialSignatureKind {
    fn from(value: u64) -> Self {
        match value {
            0 => PartialSignatureKind::PostConsensus,
            1 => PartialSignatureKind::RandaoPartialSig,
            2 => PartialSignatureKind::SelectionProofPartialSig,
            3 => PartialSignatureKind::ContributionProofs,
            4 => PartialSignatureKind::ValidatorRegistration,
            5 => PartialSignatureKind::VoluntaryExit,
            _ => panic!("Invalid PartialSignatureKind value: {value}"),
        }
    }
}

impl From<PartialSignatureKind> for u64 {
    fn from(kind: PartialSignatureKind) -> Self {
        kind as u64
    }
}

const U64_SIZE: usize = 8; // u64 is 8 bytes

impl Encode for PartialSignatureKind {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&(*self as u64).to_le_bytes());
    }

    fn ssz_fixed_len() -> usize {
        U64_SIZE
    }

    fn ssz_bytes_len(&self) -> usize {
        U64_SIZE
    }
}

impl Decode for PartialSignatureKind {
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
        match value {
            0..=5 => Ok(value.into()),
            _ => Err(DecodeError::NoMatchingVariant),
        }
    }
}

impl TreeHash for PartialSignatureKind {
    fn tree_hash_type() -> TreeHashType {
        TreeHashType::Basic
    }

    fn tree_hash_packed_encoding(&self) -> PackedEncoding {
        let value = *self as u64;
        value.tree_hash_packed_encoding()
    }

    fn tree_hash_packing_factor() -> usize {
        u64::tree_hash_packing_factor()
    }

    fn tree_hash_root(&self) -> tree_hash::Hash256 {
        let value = *self as u64;
        value.tree_hash_root()
    }
}

// A partial signature specific message
#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash, Deserialize)]
pub struct PartialSignatureMessages {
    #[serde(
        rename = "Type",
        deserialize_with = "deserialize_partial_signature_kind"
    )]
    pub kind: PartialSignatureKind,
    #[serde(rename = "Slot", deserialize_with = "deserialize_slot")]
    pub slot: Slot,
    #[serde(rename = "Messages")]
    pub messages: VariableList<PartialSignatureMessage, PartialSignatureMessagesLen>,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash, Deserialize)]
pub struct PartialSignatureMessage {
    #[serde(
        rename = "PartialSignature",
        deserialize_with = "deserialize_signature"
    )]
    pub partial_signature: Signature,
    #[serde(rename = "SigningRoot", deserialize_with = "deserialize_hash256")]
    pub signing_root: Hash256,
    #[serde(rename = "Signer")]
    pub signer: OperatorId,
    #[serde(
        rename = "ValidatorIndex",
        deserialize_with = "deserialize_validator_index"
    )]
    pub validator_index: ValidatorIndex,
}

#[derive(Debug, PartialEq)]
pub enum PartialSignatureError {
    NoMessages,
    InconsistentSigners,
    ZeroSigner,
}

impl PartialSignatureMessages {
    /// Validates the partial signature messages
    pub fn validate(&self) -> Result<(), PartialSignatureError> {
        // Must have at least one message
        if self.messages.is_empty() {
            return Err(PartialSignatureError::NoMessages);
        }

        // Get the signer from the first message
        let signer = self.messages[0].signer;

        // Validate each message and check consistency
        for message in &self.messages {
            // Check signer consistency
            if message.signer != signer {
                return Err(PartialSignatureError::InconsistentSigners);
            }

            // Validate individual message
            message.validate()?;
        }

        Ok(())
    }
}

impl PartialSignatureMessage {
    /// Validates an individual partial signature message
    pub fn validate(&self) -> Result<(), PartialSignatureError> {
        // Signer ID 0 is not allowed
        if self.signer.0 == 0 {
            return Err(PartialSignatureError::ZeroSigner);
        }

        Ok(())
    }
}
