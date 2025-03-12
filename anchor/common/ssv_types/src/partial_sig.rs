use crate::{OperatorId, ValidatorIndex};
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use types::{Hash256, Signature, Slot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartialSignatureKind {
    // PostConsensusPartialSig is a partial signature over a decided duty (attestation data, block, etc)
    PostConsensus = 0,
    // RandaoPartialSig is a partial signature over randao reveal
    RandaoPartialSig = 1,
    // SelectionProofPartialSig is a partial signature for aggregator selection proof
    SelectionProofPartialSig = 2,
    // ContributionProofs is the partial selection proofs for sync committee contributions (it's an array of sigs)
    ContributionProofs = 3,
    // ValidatorRegistrationPartialSig is a partial signature over a ValidatorRegistration object
    ValidatorRegistration = 4,
    // VoluntaryExitPartialSig is a partial signature over a VoluntaryExit object
    VoluntaryExit = 5,
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

// A partial signature specific message
#[derive(Clone, Debug, Encode, Decode)]
pub struct PartialSignatureMessages {
    pub kind: PartialSignatureKind,
    pub slot: Slot,
    pub messages: Vec<PartialSignatureMessage>,
}

#[derive(Clone, Debug, Encode, Decode)]
pub struct PartialSignatureMessage {
    pub partial_signature: Signature,
    pub signing_root: Hash256,
    pub signer: OperatorId,
    pub validator_index: ValidatorIndex,
}
