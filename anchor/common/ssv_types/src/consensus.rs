use std::{
    fmt::{Debug, Formatter},
    hash::Hash,
    ops::Deref,
};

use derive_more::{From, Into};
use sha2::{Digest, Sha256};
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use types::{
    Checkpoint, CommitteeIndex, EthSpec, ForkName, Hash256, PublicKeyBytes, Signature, Slot,
    SyncCommitteeContribution, VariableList,
    typenum::{U13, U56},
};

use crate::{ValidatorIndex, message::*};
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
//  PartialSigMsg      PartialSignatureMessages SSZ

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
    /// If this is a consensus message, fulldata contains the beacon data that is being agreed
    /// upon. Otherwise, it is empty.
    pub full_data: Vec<u8>,
}

/// A QBFT specific message
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct QbftMessage {
    pub qbft_message_type: QbftMessageType,
    pub height: u64,
    pub round: u64,
    pub identifier: VariableList<u8, U56>, /* TODO: address redundant typing due to ssz_max
                                            * encoding in go-client */
    pub root: Hash256,
    pub data_round: u64,
    pub round_change_justification: Vec<SignedSSVMessage>, // always without full_data
    pub prepare_justification: Vec<SignedSSVMessage>,      // always without full_data
}

impl Debug for QbftMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QbftMessage")
            .field("qbft_message_type", &self.qbft_message_type)
            .field("height", &self.height)
            .field("round", &self.round)
            .field("identifier", &hex::encode(self.identifier.deref()))
            .field("root", &self.root)
            .field("data_round", &self.data_round)
            .field(
                "round_change_justification",
                &self.round_change_justification,
            )
            .field("prepare_justification", &self.prepare_justification)
            .finish()
    }
}

/// Different states the QBFT Message may represent
#[derive(Clone, Debug, PartialEq, PartialOrd, Copy)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
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
        // TODO: validate proposed values
        // https://github.com/sigp/anchor/issues/258
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

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
#[ssz(struct_behaviour = "transparent")]
pub struct BeaconRole(u64);

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

/// Wrapper for [`ForkName`] to allow custom encoding/decoding used by SSV.
///
/// `ForkName` is encoded by starting from 0 for `Phase0` and increasing by 1 for each fork.
/// This type encodes starting from 1.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, From, Into)]
pub struct DataVersion(ForkName);

impl Encode for DataVersion {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let num: u64 = match self.0 {
            ForkName::Base => 1,
            ForkName::Altair => 2,
            ForkName::Bellatrix => 3,
            ForkName::Capella => 4,
            ForkName::Deneb => 5,
            ForkName::Electra => 6,
            ForkName::Fulu => 7,
        };
        num.ssz_append(buf)
    }

    fn ssz_fixed_len() -> usize {
        <u64 as Encode>::ssz_fixed_len()
    }

    fn ssz_bytes_len(&self) -> usize {
        u64::ssz_bytes_len(&0)
    }
}

impl Decode for DataVersion {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        <u64 as Decode>::ssz_fixed_len()
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let num = u64::from_ssz_bytes(bytes)?;
        Ok(DataVersion(match num {
            1 => ForkName::Base,
            2 => ForkName::Altair,
            3 => ForkName::Bellatrix,
            4 => ForkName::Capella,
            5 => ForkName::Deneb,
            6 => ForkName::Electra,
            7 => ForkName::Fulu,
            _ => return Err(DecodeError::NoMatchingVariant),
        }))
    }
}

#[derive(Clone, Debug, TreeHash, Encode, Decode)]
pub struct Contribution<E: EthSpec> {
    pub selection_proof_sig: Signature,
    pub contribution: SyncCommitteeContribution<E>,
}

#[derive(Clone, Debug, TreeHash, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct BeaconVote {
    pub block_root: Hash256,
    pub source: Checkpoint,
    pub target: Checkpoint,
}

impl QbftData for BeaconVote {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let bytes = self.as_ssz_bytes();

        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        Hash256::from(hash)
    }

    fn validate(&self) -> bool {
        // TODO: validate proposed values
        // https://github.com/sigp/anchor/issues/258
        true
    }
}
