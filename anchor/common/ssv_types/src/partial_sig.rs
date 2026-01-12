use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use types::{
    Hash256, Signature, Slot, VariableList,
    typenum::{Sum, U512, U1000},
};

use crate::{OperatorId, ValidatorIndex};

/// Maximum number of partial signature messages: 1512
/// Calculated as 1000 + 512 = 1512
pub type PartialSignatureMessagesLen = Sum<U1000, U512>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    // AggregatorCommitteePartialSig is a partial signature for combined aggregator and sync
    // committee selection proofs (committee-based batching)
    AggregatorCommitteePartialSig = 6,
}

impl TryFrom<u64> for PartialSignatureKind {
    type Error = ();

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(PartialSignatureKind::PostConsensus),
            1 => Ok(PartialSignatureKind::RandaoPartialSig),
            2 => Ok(PartialSignatureKind::SelectionProofPartialSig),
            3 => Ok(PartialSignatureKind::ContributionProofs),
            4 => Ok(PartialSignatureKind::ValidatorRegistration),
            5 => Ok(PartialSignatureKind::VoluntaryExit),
            6 => Ok(PartialSignatureKind::AggregatorCommitteePartialSig),
            _ => Err(()),
        }
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
        value.try_into().map_err(|_| DecodeError::NoMatchingVariant)
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
#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
pub struct PartialSignatureMessages {
    pub kind: PartialSignatureKind,
    pub slot: Slot,
    pub messages: VariableList<PartialSignatureMessage, PartialSignatureMessagesLen>,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
pub struct PartialSignatureMessage {
    pub partial_signature: Signature,
    pub signing_root: Hash256,
    pub signer: OperatorId,
    pub validator_index: ValidatorIndex,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssz::{Decode, Encode};

    #[test]
    fn test_partial_signature_kind_ssz_encoding() {
        // Test each variant encodes to the correct u64 little-endian bytes
        let test_cases = vec![
            (PartialSignatureKind::PostConsensus, [0, 0, 0, 0, 0, 0, 0, 0]),
            (PartialSignatureKind::RandaoPartialSig, [1, 0, 0, 0, 0, 0, 0, 0]),
            (PartialSignatureKind::SelectionProofPartialSig, [2, 0, 0, 0, 0, 0, 0, 0]),
            (PartialSignatureKind::ContributionProofs, [3, 0, 0, 0, 0, 0, 0, 0]),
            (PartialSignatureKind::ValidatorRegistration, [4, 0, 0, 0, 0, 0, 0, 0]),
            (PartialSignatureKind::VoluntaryExit, [5, 0, 0, 0, 0, 0, 0, 0]),
            (PartialSignatureKind::AggregatorCommitteePartialSig, [6, 0, 0, 0, 0, 0, 0, 0]),
        ];

        for (kind, expected_bytes) in test_cases {
            // Test encoding
            let encoded = kind.as_ssz_bytes();
            assert_eq!(encoded, expected_bytes, "Encoding failed for {:?}", kind);

            // Test decoding
            let decoded = PartialSignatureKind::from_ssz_bytes(&expected_bytes).unwrap();
            assert_eq!(decoded, kind, "Decoding failed for {:?}", kind);

            // Test roundtrip
            let roundtrip = PartialSignatureKind::from_ssz_bytes(&kind.as_ssz_bytes()).unwrap();
            assert_eq!(roundtrip, kind, "Roundtrip failed for {:?}", kind);
        }
    }

    #[test]
    fn test_partial_signature_kind_invalid_decoding() {
        // Test that invalid values return errors
        let invalid_values = vec![
            [7, 0, 0, 0, 0, 0, 0, 0],  // Invalid variant
            [255, 255, 255, 255, 255, 255, 255, 255],  // Max u64
        ];

        for invalid_bytes in invalid_values {
            let result = PartialSignatureKind::from_ssz_bytes(&invalid_bytes);
            assert!(result.is_err(), "Should fail to decode invalid value: {:?}", invalid_bytes);
        }

        // Test that wrong length bytes return error
        let wrong_length = vec![0u8; 4];  // Only 4 bytes instead of 8
        let result = PartialSignatureKind::from_ssz_bytes(&wrong_length);
        assert!(result.is_err(), "Should fail to decode with wrong byte length");
    }

    #[test]
    fn test_partial_signature_kind_aggregator_committee_variant() {
        // Specific test for the new AggregatorCommitteePartialSig variant
        let variant = PartialSignatureKind::AggregatorCommitteePartialSig;

        // Test encoding to value 6
        let encoded_bytes = variant.as_ssz_bytes();
        assert_eq!(encoded_bytes, [6, 0, 0, 0, 0, 0, 0, 0],
            "AggregatorCommitteePartialSig should encode to 6 as u64 little-endian");

        // Test that it has the correct discriminant value
        assert_eq!(variant as u64, 6, "AggregatorCommitteePartialSig should have value 6");

        // Test decoding from value 6
        let decoded = PartialSignatureKind::from_ssz_bytes(&[6, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(decoded, PartialSignatureKind::AggregatorCommitteePartialSig,
            "Should decode value 6 to AggregatorCommitteePartialSig");

        // Test TryFrom<u64>
        let from_u64 = PartialSignatureKind::try_from(6u64).unwrap();
        assert_eq!(from_u64, PartialSignatureKind::AggregatorCommitteePartialSig,
            "Should convert u64 value 6 to AggregatorCommitteePartialSig");
    }

    #[test]
    fn test_partial_signature_kind_fixed_length() {
        // Verify SSZ fixed length properties
        assert!(<PartialSignatureKind as Encode>::is_ssz_fixed_len(), "Should be fixed length");
        assert_eq!(<PartialSignatureKind as Encode>::ssz_fixed_len(), 8, "Fixed length should be 8 bytes");

        // Test each variant has the same bytes length
        let variants = vec![
            PartialSignatureKind::PostConsensus,
            PartialSignatureKind::RandaoPartialSig,
            PartialSignatureKind::SelectionProofPartialSig,
            PartialSignatureKind::ContributionProofs,
            PartialSignatureKind::ValidatorRegistration,
            PartialSignatureKind::VoluntaryExit,
            PartialSignatureKind::AggregatorCommitteePartialSig,
        ];

        for variant in variants {
            assert_eq!(variant.ssz_bytes_len(), 8, "All variants should have 8 byte length");
            assert_eq!(variant.as_ssz_bytes().len(), 8, "Encoded bytes should be 8 bytes");
        }
    }
}
