use bls::Signature;
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use ssz_types::VariableList;
use thiserror::Error;
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use typenum::{Prod, Sum, U3, U4, U512, U1000};
use types::{Hash256, Slot};

#[cfg(feature = "serde")]
use crate::deserializers::*;
use crate::{OperatorId, ValidatorIndex};

/// Maximum number of `PartialSignatureMessage`s: 5048
/// Worst case scenario for a committee with 3000 validators:
/// every validator has an aggregation duty (3000) +
/// every validator is in sync committee and is a contributor to all 4 subnets (512 * 4 = 2048)
/// Calculated as 3000 + 512 * 4 = 5048
pub type PartialSignatureMessagesLen = Sum<Prod<U3, U1000>, Prod<U512, U4>>;

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
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
pub struct PartialSignatureMessages {
    #[cfg_attr(
        feature = "serde",
        serde(
            rename = "Type",
            deserialize_with = "deserialize_partial_signature_kind"
        )
    )]
    pub kind: PartialSignatureKind,
    #[cfg_attr(
        feature = "serde",
        serde(rename = "Slot", deserialize_with = "deserialize_slot")
    )]
    pub slot: Slot,
    #[cfg_attr(feature = "serde", serde(rename = "Messages"))]
    pub messages: VariableList<PartialSignatureMessage, PartialSignatureMessagesLen>,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
pub struct PartialSignatureMessage {
    #[cfg_attr(
        feature = "serde",
        serde(
            rename = "PartialSignature",
            deserialize_with = "deserialize_signature"
        )
    )]
    pub partial_signature: Signature,
    #[cfg_attr(
        feature = "serde",
        serde(rename = "SigningRoot", deserialize_with = "deserialize_hash256")
    )]
    pub signing_root: Hash256,
    #[cfg_attr(feature = "serde", serde(rename = "Signer"))]
    pub signer: OperatorId,
    #[cfg_attr(
        feature = "serde",
        serde(
            rename = "ValidatorIndex",
            deserialize_with = "deserialize_validator_index"
        )
    )]
    pub validator_index: ValidatorIndex,
}

/// Errors from `PartialSignatureMessages::validate()`.
///
/// Mirrors Go's `PartialSignatureMessages.Validate()` error conditions.
#[derive(Debug, Error)]
pub enum PartialSignatureMessagesError {
    #[error("no partial signature messages")]
    Empty,
    #[error("inconsistent signers")]
    InconsistentSigners,
    #[error("signer ID 0 not allowed")]
    ZeroSigner,
}

impl PartialSignatureMessages {
    /// Validate the message structure.
    ///
    /// Mirrors Go's `PartialSignatureMessages.Validate()`:
    /// 1. Messages must not be empty
    /// 2. All message signers must be the same
    /// 3. No signer may have ID 0
    pub fn validate(&self) -> Result<(), PartialSignatureMessagesError> {
        let first = self
            .messages
            .first()
            .ok_or(PartialSignatureMessagesError::Empty)?;

        for m in self.messages.iter() {
            if m.signer != first.signer {
                return Err(PartialSignatureMessagesError::InconsistentSigners);
            }
            if m.signer == OperatorId(0) {
                return Err(PartialSignatureMessagesError::ZeroSigner);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tree_hash::TreeHash;

    use super::*;

    // ═══════════════════════════════════════════════════════════════════════════════
    // PartialSignatureKind SSZ Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn partial_signature_kind_ssz_roundtrip_all_variants() {
        let variants = [
            PartialSignatureKind::PostConsensus,
            PartialSignatureKind::RandaoPartialSig,
            PartialSignatureKind::SelectionProofPartialSig,
            PartialSignatureKind::ContributionProofs,
            PartialSignatureKind::ValidatorRegistration,
            PartialSignatureKind::VoluntaryExit,
            PartialSignatureKind::AggregatorCommitteePartialSig,
        ];

        for variant in variants {
            let encoded = variant.as_ssz_bytes();
            let decoded = PartialSignatureKind::from_ssz_bytes(&encoded).unwrap();
            assert_eq!(variant, decoded, "Roundtrip failed for {:?}", variant);
        }
    }

    #[test]
    fn partial_signature_kind_is_fixed_size() {
        assert!(
            <PartialSignatureKind as Encode>::is_ssz_fixed_len(),
            "PartialSignatureKind should be fixed-size SSZ"
        );
        assert_eq!(
            <PartialSignatureKind as Encode>::ssz_fixed_len(),
            8,
            "PartialSignatureKind should be 8 bytes"
        );
    }

    #[test]
    fn partial_signature_kind_ssz_byte_layout() {
        let test_cases = [
            (PartialSignatureKind::PostConsensus, 0u64),
            (PartialSignatureKind::RandaoPartialSig, 1u64),
            (PartialSignatureKind::SelectionProofPartialSig, 2u64),
            (PartialSignatureKind::ContributionProofs, 3u64),
            (PartialSignatureKind::ValidatorRegistration, 4u64),
            (PartialSignatureKind::VoluntaryExit, 5u64),
            (PartialSignatureKind::AggregatorCommitteePartialSig, 6u64),
        ];

        for (variant, expected_value) in test_cases {
            let encoded = variant.as_ssz_bytes();
            assert_eq!(encoded.len(), 8, "All variants should encode to 8 bytes");
            assert_eq!(
                encoded,
                expected_value.to_le_bytes().to_vec(),
                "{:?} should encode to {} in little-endian",
                variant,
                expected_value
            );
        }
    }

    #[test]
    fn partial_signature_kind_ssz_decode_invalid_variant() {
        let invalid_value = 7u64.to_le_bytes();
        let result = PartialSignatureKind::from_ssz_bytes(&invalid_value);
        assert!(matches!(result, Err(DecodeError::NoMatchingVariant)));
    }

    #[test]
    fn partial_signature_kind_ssz_decode_invalid_length() {
        // Too short
        let short_bytes = vec![0u8; 7];
        let result = PartialSignatureKind::from_ssz_bytes(&short_bytes);
        assert!(matches!(
            result,
            Err(DecodeError::InvalidByteLength {
                len: 7,
                expected: 8
            })
        ));

        // Too long
        let long_bytes = vec![0u8; 9];
        let result = PartialSignatureKind::from_ssz_bytes(&long_bytes);
        assert!(matches!(
            result,
            Err(DecodeError::InvalidByteLength {
                len: 9,
                expected: 8
            })
        ));
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // PartialSignatureKind TryFrom Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn partial_signature_kind_try_from_u64_all_valid_values() {
        assert_eq!(
            PartialSignatureKind::try_from(0u64).unwrap(),
            PartialSignatureKind::PostConsensus
        );
        assert_eq!(
            PartialSignatureKind::try_from(1u64).unwrap(),
            PartialSignatureKind::RandaoPartialSig
        );
        assert_eq!(
            PartialSignatureKind::try_from(2u64).unwrap(),
            PartialSignatureKind::SelectionProofPartialSig
        );
        assert_eq!(
            PartialSignatureKind::try_from(3u64).unwrap(),
            PartialSignatureKind::ContributionProofs
        );
        assert_eq!(
            PartialSignatureKind::try_from(4u64).unwrap(),
            PartialSignatureKind::ValidatorRegistration
        );
        assert_eq!(
            PartialSignatureKind::try_from(5u64).unwrap(),
            PartialSignatureKind::VoluntaryExit
        );
        assert_eq!(
            PartialSignatureKind::try_from(6u64).unwrap(),
            PartialSignatureKind::AggregatorCommitteePartialSig
        );
    }

    #[test]
    fn partial_signature_kind_try_from_u64_invalid_values() {
        assert!(PartialSignatureKind::try_from(7u64).is_err());
        assert!(PartialSignatureKind::try_from(100u64).is_err());
        assert!(PartialSignatureKind::try_from(u64::MAX).is_err());
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // PartialSignatureKind TreeHash Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn partial_signature_kind_tree_hash_deterministic() {
        let variant = PartialSignatureKind::AggregatorCommitteePartialSig;
        let hash1 = variant.tree_hash_root();
        let hash2 = variant.tree_hash_root();
        assert_eq!(hash1, hash2, "Tree hash should be deterministic");
    }

    #[test]
    fn partial_signature_kind_tree_hash_differs_per_variant() {
        let variants = [
            PartialSignatureKind::PostConsensus,
            PartialSignatureKind::RandaoPartialSig,
            PartialSignatureKind::SelectionProofPartialSig,
            PartialSignatureKind::ContributionProofs,
            PartialSignatureKind::ValidatorRegistration,
            PartialSignatureKind::VoluntaryExit,
            PartialSignatureKind::AggregatorCommitteePartialSig,
        ];

        let hashes: Vec<_> = variants.iter().map(|v| v.tree_hash_root()).collect();

        // Verify all hashes are unique
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(
                    hashes[i], hashes[j],
                    "{:?} and {:?} should have different tree hashes",
                    variants[i], variants[j]
                );
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // AggregatorCommitteePartialSig Specific Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn aggregator_committee_partial_sig_variant_value() {
        assert_eq!(
            PartialSignatureKind::AggregatorCommitteePartialSig as u64,
            6,
            "AggregatorCommitteePartialSig should have discriminant value 6"
        );
    }

    #[test]
    fn aggregator_committee_partial_sig_ssz_encoding() {
        let variant = PartialSignatureKind::AggregatorCommitteePartialSig;
        let encoded = variant.as_ssz_bytes();

        // Should encode as 6 in little-endian format
        assert_eq!(
            encoded,
            vec![6, 0, 0, 0, 0, 0, 0, 0],
            "AggregatorCommitteePartialSig should encode as [6, 0, 0, 0, 0, 0, 0, 0]"
        );

        // Should decode back to the same variant
        let decoded = PartialSignatureKind::from_ssz_bytes(&encoded).unwrap();
        assert_eq!(decoded, PartialSignatureKind::AggregatorCommitteePartialSig);
    }
}
