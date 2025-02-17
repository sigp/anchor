use crate::message::*;
use crate::{OperatorId, ValidatorIndex};
use sha2::{Digest, Sha256};
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use std::fmt::Debug;
use std::hash::Hash;
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use types::typenum::U13;
use types::{
    AggregateAndProof, AggregateAndProofBase, AggregateAndProofElectra, BeaconBlock,
    BlindedBeaconBlock, Checkpoint, CommitteeIndex, EthSpec, Hash256, PublicKeyBytes, Signature,
    Slot, SyncCommitteeContribution, VariableList,
};

//                          UnsignedSSVMessage
//            ----------------------------------------------
//            |                                            |
//            |                                            |
//          SSVMessage                                 FullData
//     ---------------------                          ----------
//     |                   |              ValidatorConsensusData/BeaconVote SSZ
//     |                   |
//   MsgType            FullData
//  ---------          -----------
//  ConsensusMsg       QBFTMessage SSZ
//  PartialSigMsg      PartialSignatureMessage SSZ

pub trait QbftData: Debug + Clone + Encode + Decode {
    type Hash: Debug + Clone + Eq + Hash;

    fn hash(&self) -> Self::Hash;
    fn validate(&self) -> bool;
}

/// A SSV Message that has not been signed yet.
#[derive(Clone, Debug, Encode)]
pub struct UnsignedSSVMessage {
    /// The SSV Message to be send. This is either a consensus message which contains a serialized
    /// QbftMessage, or a partial signature message which contains a PartialSignatureMessage
    pub ssv_message: SSVMessage,
    /// If this is a consensus message, fulldata contains the beacon data that is being agreed upon.
    /// Otherwise, it is empty.
    pub full_data: Vec<u8>,
}

/// A QBFT specific message
#[derive(Clone, Debug, Encode, Decode)]
pub struct QbftMessage {
    pub qbft_message_type: QbftMessageType,
    pub height: u64,
    pub round: u64,
    pub identifier: MessageID,
    pub root: Hash256,
    pub data_round: u64,
    pub round_change_justification: Vec<SignedSSVMessage>, // always without full_data
    pub prepare_justification: Vec<SignedSSVMessage>,      // always without full_data
}

impl QbftMessage {
    /// Do QBFTMessage specific validation
    pub fn validate(&self) -> bool {
        if self.qbft_message_type > QbftMessageType::RoundChange {
            return false;
        }
        true
    }
}

/// Different states the QBFT Message may represent
#[derive(Clone, Debug, PartialEq, PartialOrd, Copy)]
pub enum QbftMessageType {
    Proposal = 0,
    Prepare,
    Commit,
    RoundChange,
}

impl Encode for QbftMessageType {
    // QbftMessageType is represented as a fixed-length u64
    fn is_ssz_fixed_len() -> bool {
        true
    }

    // Append the bytes representation of the enum variant
    fn ssz_append(&self, buf: &mut Vec<u8>) {
        // Convert enum variant to u64 and append bytes
        let value: u64 = match self {
            QbftMessageType::Proposal => 0,
            QbftMessageType::Prepare => 1,
            QbftMessageType::Commit => 2,
            QbftMessageType::RoundChange => 3,
        };
        buf.extend_from_slice(&value.to_le_bytes());
    }

    // Fixed length is 8 bytes (size of u64)
    fn ssz_fixed_len() -> usize {
        8
    }

    // Actual length is always 8 bytes
    fn ssz_bytes_len(&self) -> usize {
        8
    }
}

impl Decode for QbftMessageType {
    // QbftMessageType is always fixed length
    fn is_ssz_fixed_len() -> bool {
        true
    }

    // Fixed length is 8 bytes (size of u64)
    fn ssz_fixed_len() -> usize {
        8
    }

    // Convert bytes back into enum variant
    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        // Verify we have exactly 8 bytes
        if bytes.len() != 8 {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: 8,
            });
        }

        // Convert bytes to u64
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        let value = u64::from_le_bytes(array);

        // Convert value back to enum variant
        match value {
            0 => Ok(QbftMessageType::Proposal),
            1 => Ok(QbftMessageType::Prepare),
            2 => Ok(QbftMessageType::Commit),
            3 => Ok(QbftMessageType::RoundChange),
            _ => Err(DecodeError::NoMatchingVariant),
        }
    }
}

// A partial signature specific message
#[derive(Clone, Debug)]
pub struct PartialSignatureMessage {
    pub partial_signature: Signature,
    pub signing_root: Hash256,
    pub signer: OperatorId,
    pub validator_index: ValidatorIndex,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct ValidatorConsensusData {
    pub duty: ValidatorDuty,
    pub version: DataVersion,
    pub data_ssz: Vec<u8>,
}

impl QbftData for ValidatorConsensusData {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let bytes = self.as_ssz_bytes();

        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        Hash256::from(hash)
    }

    fn validate(&self) -> bool {
        // todo!(). What does proper validation look like??
        true
    }
}

#[derive(Clone, Debug, TreeHash, PartialEq, Encode, Decode)]
pub struct ValidatorDuty {
    pub r#type: BeaconRole,
    pub pub_key: PublicKeyBytes,
    pub slot: Slot,
    pub validator_index: ValidatorIndex,
    pub committee_index: CommitteeIndex,
    pub committee_length: u64,
    pub committees_at_slot: u64,
    pub validator_committee_index: u64,
    pub validator_sync_committee_indices: VariableList<u64, U13>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BeaconRole(u64);

// BeaconRole SSZ implementation
impl Encode for BeaconRole {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.0.to_le_bytes());
    }

    fn ssz_fixed_len() -> usize {
        8
    }

    fn ssz_bytes_len(&self) -> usize {
        8
    }
}

impl Decode for BeaconRole {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        8
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != 8 {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: 8,
            });
        }

        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(BeaconRole(u64::from_le_bytes(array)))
    }
}

pub const BEACON_ROLE_ATTESTER: BeaconRole = BeaconRole(0);
pub const BEACON_ROLE_AGGREGATOR: BeaconRole = BeaconRole(1);
pub const BEACON_ROLE_PROPOSER: BeaconRole = BeaconRole(2);
pub const BEACON_ROLE_SYNC_COMMITTEE: BeaconRole = BeaconRole(3);
pub const BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION: BeaconRole = BeaconRole(4);
pub const BEACON_ROLE_VALIDATOR_REGISTRATION: BeaconRole = BeaconRole(5);
pub const BEACON_ROLE_VOLUNTARY_EXIT: BeaconRole = BeaconRole(6);
pub const BEACON_ROLE_UNKNOWN: BeaconRole = BeaconRole(u64::MAX);

impl TreeHash for BeaconRole {
    fn tree_hash_type() -> TreeHashType {
        u64::tree_hash_type()
    }

    fn tree_hash_packed_encoding(&self) -> PackedEncoding {
        self.0.tree_hash_packed_encoding()
    }

    fn tree_hash_packing_factor() -> usize {
        u64::tree_hash_packing_factor()
    }

    fn tree_hash_root(&self) -> tree_hash::Hash256 {
        self.0.tree_hash_root()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataVersion(u64);

// DataVersion SSZ implementation
impl Encode for DataVersion {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        // DataVersion is represented as u64 internally
        buf.extend_from_slice(&self.0.to_le_bytes());
    }

    fn ssz_fixed_len() -> usize {
        8 // u64 size
    }

    fn ssz_bytes_len(&self) -> usize {
        8
    }
}

impl Decode for DataVersion {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        8 // u64 size
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != 8 {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: 8,
            });
        }

        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(DataVersion(u64::from_le_bytes(array)))
    }
}

pub const DATA_VERSION_UNKNOWN: DataVersion = DataVersion(0);
pub const DATA_VERSION_PHASE0: DataVersion = DataVersion(1);
pub const DATA_VERSION_ALTAIR: DataVersion = DataVersion(2);
pub const DATA_VERSION_BELLATRIX: DataVersion = DataVersion(3);
pub const DATA_VERSION_CAPELLA: DataVersion = DataVersion(4);
pub const DATA_VERSION_DENEB: DataVersion = DataVersion(5);

impl TreeHash for DataVersion {
    fn tree_hash_type() -> TreeHashType {
        u64::tree_hash_type()
    }

    fn tree_hash_packed_encoding(&self) -> PackedEncoding {
        self.0.tree_hash_packed_encoding()
    }

    fn tree_hash_packing_factor() -> usize {
        u64::tree_hash_packing_factor()
    }

    fn tree_hash_root(&self) -> tree_hash::Hash256 {
        self.0.tree_hash_root()
    }
}

#[derive(Clone, Debug, TreeHash, Encode)]
#[tree_hash(enum_behaviour = "transparent")]
#[ssz(enum_behaviour = "transparent")]
pub enum DataSsz<E: EthSpec> {
    AggregateAndProof(AggregateAndProof<E>),
    BlindedBeaconBlock(BlindedBeaconBlock<E>),
    BeaconBlock(BeaconBlock<E>),
    Contributions(VariableList<Contribution<E>, U13>),
}

impl<E: EthSpec> DataSsz<E> {
    /// SSZ deserialization that tries all possible variants
    pub fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        // 1. Try BeaconBlock variants
        if let Ok(block) = BeaconBlock::any_from_ssz_bytes(bytes) {
            return Ok(Self::BeaconBlock(block));
        }

        // 2. Try BlindedBeaconBlock
        if let Ok(blinded) = BlindedBeaconBlock::any_from_ssz_bytes(bytes) {
            return Ok(Self::BlindedBeaconBlock(blinded));
        }

        // 3. Handle AggregateAndProof variants explicitly
        if let Ok(base) = AggregateAndProofBase::<E>::from_ssz_bytes(bytes) {
            return Ok(Self::AggregateAndProof(AggregateAndProof::Base(base)));
        }
        if let Ok(electra) = AggregateAndProofElectra::<E>::from_ssz_bytes(bytes) {
            return Ok(Self::AggregateAndProof(AggregateAndProof::Electra(electra)));
        }

        // 4. Try Contributions
        if let Ok(contributions) = VariableList::<Contribution<E>, U13>::from_ssz_bytes(bytes) {
            return Ok(Self::Contributions(contributions));
        }

        Err(ssz::DecodeError::BytesInvalid(
            "Failed to decode as any DataSsz variant".into(),
        ))
    }
}

#[derive(Clone, Debug, TreeHash, Encode, Decode)]
pub struct Contribution<E: EthSpec> {
    pub selection_proof_sig: Signature,
    pub contribution: SyncCommitteeContribution<E>,
}

#[derive(Clone, Debug, TreeHash, PartialEq, Eq, Encode, Decode)]
pub struct BeaconVote {
    pub block_root: Hash256,
    pub source: Checkpoint,
    pub target: Checkpoint,
}

impl QbftData for BeaconVote {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        self.tree_hash_root()
    }

    fn validate(&self) -> bool {
        // todo!(). What does proper validation look like??
        true
    }
}
