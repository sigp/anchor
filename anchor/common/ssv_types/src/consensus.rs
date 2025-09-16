use std::{
    collections::HashMap,
    fmt::{Debug, DebugStruct, Display, Formatter},
    hash::Hash,
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
};

use derive_more::{From, Into};
use eth2::types::FullBlockContents;
use sha2::{Digest, Sha256};
use slashing_protection::{NotSafe, SlashingDatabase};
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use thiserror::Error;
use tracing::warn;
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use types::{
    AggregateAndProofBase, AggregateAndProofElectra, AttestationData, BlindedBeaconBlock,
    ChainSpec, Checkpoint, CommitteeIndex, Domain, EthSpec, ForkName, Hash256, PublicKeyBytes,
    Signature, Slot, SyncCommitteeContribution, VariableList,
    typenum::{Pow, Prod, Sum, U2, U3, U5, U13, U23, U56, U700, U852, U1000, U10000},
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
}

pub trait QbftDataValidator<D: QbftData>: Send + Sync {
    fn validate(&self, value: &D, start_value: &D) -> bool;
}

#[derive(Debug)]
pub struct NoDataValidation;
impl<D: QbftData> QbftDataValidator<D> for NoDataValidation {
    fn validate(&self, _value: &D, _start_value: &D) -> bool {
        true
    }
}

/// ValidatorConsensusData.DataSSZ max size: 8388608 bytes (2^23)
/// This is the maximum size that the validator consensus data may be
/// Calculated as 2^23 = 8,388,608
pub type ValidatorConsensusDataLen = <U2 as Pow<U23>>::Output;

// RoundChange max size: 51852
// This is the maximum size that a round change justification may be
// Calculated as (5 * 10,000) + 1,000 + 852
pub type RoundChangeJustificationLength = Sum<Prod<U5, U10000>, Sum<U1000, U852>>;

// Justification max size: 3700
// This is the maximum size that a prepare justification may be
// Calculated as (3 * 1000) + 700
pub type PrepareJustificationLength = Sum<Prod<U3, U1000>, U700>; // 3700

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
#[derive(Debug, Clone, Encode, Decode, TreeHash)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct QbftMessage {
    pub qbft_message_type: QbftMessageType,
    pub height: u64,
    pub round: u64,
    pub identifier: VariableList<u8, U56>, /* TODO: address redundant typing due to ssz_max
                                            * encoding in go-client */
    pub root: Hash256,
    pub data_round: u64,
    // always without full data
    pub round_change_justification:
        VariableList<VariableList<u8, RoundChangeJustificationLength>, U13>,
    // always without full data
    pub prepare_justification: VariableList<VariableList<u8, PrepareJustificationLength>, U13>,
}

impl Display for QbftMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("QbftMessage");
        self.format_fields(&mut f);
        f.finish()
    }
}

impl QbftMessage {
    pub fn format_fields(&self, f: &mut DebugStruct<'_, '_>) {
        f.field("qbft_message_type", &self.qbft_message_type)
            .field("height", &self.height)
            .field("round", &self.round)
            .field("msg_id", &hex::encode(self.identifier.deref()))
            .field("root", &self.root)
            .field("data_round", &self.data_round)
            .field(
                "round_change_justification",
                &self.round_change_justification.len(),
            )
            .field("prepare_justification", &self.prepare_justification.len());
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

impl TreeHash for QbftMessageType {
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

#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
pub struct ValidatorConsensusData {
    pub duty: ValidatorDuty,
    pub version: DataVersion,
    pub data_ssz: VariableList<u8, ValidatorConsensusDataLen>,
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
}

pub struct ValidatorConsensusDataValidator<E: EthSpec> {
    slashing_database: Arc<SlashingDatabase>,
    disable_slashing_protection: bool,
    spec: Arc<ChainSpec>,
    validator_pubkey: PublicKeyBytes,
    genesis_validators_root: Hash256,
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> QbftDataValidator<ValidatorConsensusData> for ValidatorConsensusDataValidator<E> {
    fn validate(&self, value: &ValidatorConsensusData, our_value: &ValidatorConsensusData) -> bool {
        match self.do_validation(value, our_value) {
            Ok(_) => true,
            Err(err) => {
                warn!(%err, "Operator proposed invalid validator consensus data");
                false
            }
        }
    }
}

impl<E: EthSpec> ValidatorConsensusDataValidator<E> {
    pub fn new(
        slashing_database: Arc<SlashingDatabase>,
        disable_slashing_protection: bool,
        spec: Arc<ChainSpec>,
        validator_pubkey: PublicKeyBytes,
        genesis_validators_root: Hash256,
    ) -> Self {
        Self {
            slashing_database,
            disable_slashing_protection,
            spec,
            validator_pubkey,
            genesis_validators_root,
            _phantom: PhantomData,
        }
    }

    pub fn do_validation(
        &self,
        value: &ValidatorConsensusData,
        our_value: &ValidatorConsensusData,
    ) -> Result<(), DataValidationError> {
        // Check whether the slot matches
        if value.duty.slot != our_value.duty.slot {
            return Err(DataValidationError::SlotMismatch {
                expected: our_value.duty.slot,
                got: value.duty.slot,
            });
        }

        // Check if the proposed value matches our proposal candidate:
        // Type (Beacon Role) must match
        if value.duty.r#type != our_value.duty.r#type {
            return Err(DataValidationError::RoleMismatch {
                expected: our_value.duty.r#type,
                got: value.duty.r#type,
            });
        }

        // Public key must match
        if value.duty.pub_key != our_value.duty.pub_key {
            return Err(DataValidationError::PubKeyMismatch {
                expected: our_value.duty.pub_key,
                got: value.duty.pub_key,
            });
        }

        // Validator index must match
        if value.duty.validator_index != our_value.duty.validator_index {
            return Err(DataValidationError::IndexMismatch {
                expected: our_value.duty.validator_index,
                got: value.duty.validator_index,
            });
        }

        match value.duty.r#type {
            BEACON_ROLE_AGGREGATOR => {
                if value.version < DataVersion(ForkName::Electra) {
                    AggregateAndProofBase::<E>::from_ssz_bytes(&value.data_ssz)?;
                } else {
                    AggregateAndProofElectra::<E>::from_ssz_bytes(&value.data_ssz)?;
                }
            }
            BEACON_ROLE_PROPOSER => {
                self.validate_block_proposal(value)?;
            }
            BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION => {
                // There is nothing special to check for sync committee contributions.
                // We just need to ensure that the data is valid.
                Contributions::<E>::from_ssz_bytes(&value.data_ssz)?;
            }
            other => return Err(DataValidationError::InvalidDutyType(other)),
        };
        Ok(())
    }

    fn validate_block_proposal(
        &self,
        value: &ValidatorConsensusData,
    ) -> Result<(), DataValidationError> {
        let fork = ForkName::from(value.version);

        // Always do this check, even if we're not validating slashing. This is to ensure that we
        // have a decodable value.
        let header = BlindedBeaconBlock::<E>::from_ssz_bytes_for_fork(&value.data_ssz, fork)
            .map(|block| block.block_header())
            .or_else(|_| {
                FullBlockContents::<E>::from_ssz_bytes_for_fork(&value.data_ssz, fork)
                    .map(|block| block.block().block_header())
            })
            .map_err(DataValidationError::DecodeError)?;

        if !self.disable_slashing_protection {
            let epoch = header.slot.epoch(E::slots_per_epoch());

            let domain_hash = self.spec.get_domain(
                epoch,
                Domain::BeaconProposer,
                &self.spec.fork_at_epoch(epoch),
                self.genesis_validators_root,
            );

            self.slashing_database
                .preliminary_check_block_proposal(&self.validator_pubkey, &header, domain_hash)
                .map_err(DataValidationError::SlashableBlockProposal)?;
        }

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum DataValidationError {
    #[error("Unable to decode ssz in ValidatorConsensusData: {0:?}")]
    DecodeError(DecodeError),
    #[error("Invalid duty type for QBFT: {0:?}")]
    InvalidDutyType(BeaconRole),
    #[error("Slot mismatches: expected {expected}, got {got}")]
    SlotMismatch { expected: Slot, got: Slot },
    #[error("wrong beacon role type: expected {expected:?}, got {got:?}")]
    RoleMismatch {
        expected: BeaconRole,
        got: BeaconRole,
    },
    #[error("wrong validator pk: expected {expected:?}, got {got:?}")]
    PubKeyMismatch {
        expected: PublicKeyBytes,
        got: PublicKeyBytes,
    },
    #[error("wrong validator index: expected {expected:?}, got {got:?}")]
    IndexMismatch {
        expected: ValidatorIndex,
        got: ValidatorIndex,
    },
    #[error("Block proposal would be slashable: {0}")]
    SlashableBlockProposal(NotSafe),
}

impl From<DecodeError> for DataValidationError {
    fn from(err: DecodeError) -> Self {
        DataValidationError::DecodeError(err)
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

#[derive(Clone, Copy, Debug, PartialEq, Encode, Decode)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, From, Into)]
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

impl TreeHash for DataVersion {
    fn tree_hash_type() -> TreeHashType {
        TreeHashType::Basic
    }

    fn tree_hash_packed_encoding(&self) -> PackedEncoding {
        let num: u64 = match self.0 {
            ForkName::Base => 1,
            ForkName::Altair => 2,
            ForkName::Bellatrix => 3,
            ForkName::Capella => 4,
            ForkName::Deneb => 5,
            ForkName::Electra => 6,
            ForkName::Fulu => 7,
        };
        num.tree_hash_packed_encoding()
    }

    fn tree_hash_packing_factor() -> usize {
        u64::tree_hash_packing_factor()
    }

    fn tree_hash_root(&self) -> tree_hash::Hash256 {
        let num: u64 = match self.0 {
            ForkName::Base => 1,
            ForkName::Altair => 2,
            ForkName::Bellatrix => 3,
            ForkName::Capella => 4,
            ForkName::Deneb => 5,
            ForkName::Electra => 6,
            ForkName::Fulu => 7,
        };
        num.tree_hash_root()
    }
}

#[derive(Clone, Debug, TreeHash, Encode, Decode)]
pub struct Contribution<E: EthSpec> {
    pub selection_proof_sig: Signature,
    pub contribution: SyncCommitteeContribution<E>,
}

/// This type is a workaround for the fact that Go-SSV encodes lists of `Contribution` incorrectly:
/// it treats `Contribution` as if it had a variable length, but it does not. This wrapper
/// implements `Encode` and `Decode` to set `is_ssz_fixed_len` to `false` and delegates to the
/// macro impls of `Encode and `Decode` on `Contribution` for the actual serialization and
/// deserialization.
#[derive(Clone, Debug, Into, From)]
pub struct ContributionWrapper<E: EthSpec> {
    pub contribution: Contribution<E>,
}

impl<E: EthSpec> Encode for ContributionWrapper<E> {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.contribution.ssz_append(buf)
    }

    fn ssz_bytes_len(&self) -> usize {
        self.contribution.ssz_bytes_len()
    }
}

impl<E: EthSpec> Decode for ContributionWrapper<E> {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        Ok(Self {
            contribution: Contribution::from_ssz_bytes(bytes)?,
        })
    }
}

pub type Contributions<E> = VariableList<ContributionWrapper<E>, U13>;

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
}

pub struct BeaconVoteValidator<E: EthSpec> {
    slot: Slot,
    slashing_database: Arc<SlashingDatabase>,
    disable_slashing_protection: bool,
    spec: Arc<ChainSpec>,
    validator_attestation_committees: HashMap<PublicKeyBytes, u64>,
    genesis_validators_root: Hash256,
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> QbftDataValidator<BeaconVote> for BeaconVoteValidator<E> {
    fn validate(&self, value: &BeaconVote, our_value: &BeaconVote) -> bool {
        match self.do_validation(value, our_value) {
            Ok(_) => true,
            Err(err) => {
                warn!(%err, "Operator proposed invalid beacon vote");
                false
            }
        }
    }
}

impl<E: EthSpec> BeaconVoteValidator<E> {
    pub fn new(
        slot: Slot,
        slashing_database: Arc<SlashingDatabase>,
        disable_slashing_protection: bool,
        spec: Arc<ChainSpec>,
        validator_attestation_committees: HashMap<PublicKeyBytes, u64>,
        genesis_validators_root: Hash256,
    ) -> Self {
        Self {
            slot,
            slashing_database,
            disable_slashing_protection,
            spec,
            validator_attestation_committees,
            genesis_validators_root,
            _phantom: PhantomData,
        }
    }

    pub fn do_validation(
        &self,
        value: &BeaconVote,
        _our_value: &BeaconVote,
    ) -> Result<(), BeaconVoteValidationError> {
        // Check target epoch is not too far in the future
        let current_epoch = self.slot.epoch(E::slots_per_epoch());
        if value.target.epoch > current_epoch + 1 {
            return Err(BeaconVoteValidationError::FarFutureTargetEpoch(format!(
                "current: {}, target: {}",
                current_epoch.as_u64(),
                value.target.epoch.as_u64()
            )));
        }

        // Check source epoch < target epoch
        if value.source.epoch >= value.target.epoch {
            return Err(BeaconVoteValidationError::TargetNotAfterSource(format!(
                "source {} >= target {}",
                value.source.epoch.as_u64(),
                value.target.epoch.as_u64()
            )));
        }

        // Check slashing protection for all validator public keys
        if !self.disable_slashing_protection {
            self.check_attestation_slashing(value)?;
        }

        Ok(())
    }

    fn check_attestation_slashing(
        &self,
        value: &BeaconVote,
    ) -> Result<(), BeaconVoteValidationError> {
        // Create attestation data for slashing protection check
        let mut attestation_data = AttestationData {
            slot: self.slot,
            index: 0, // Will be individually set below
            beacon_block_root: value.block_root,
            source: value.source,
            target: value.target,
        };

        let epoch = self.slot.epoch(E::slots_per_epoch());

        let domain_hash = self.spec.get_domain(
            epoch,
            Domain::BeaconAttester,
            &self.spec.fork_at_epoch(epoch),
            self.genesis_validators_root,
        );

        for (validator_pubkey, committee_index) in &self.validator_attestation_committees {
            attestation_data.index = *committee_index;
            self.slashing_database
                .preliminary_check_attestation(validator_pubkey, &attestation_data, domain_hash)
                .map_err(BeaconVoteValidationError::SlashableAttestation)?;
        }

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum BeaconVoteValidationError {
    #[error("Unable to validate, bad slot clock")]
    BadSlotClock,
    #[error("Target epoch is too far in future: {0}")]
    FarFutureTargetEpoch(String),
    #[error("Invalid epoch order: {0}")]
    TargetNotAfterSource(String),
    #[error("Attestation would be slashable: {0}")]
    SlashableAttestation(NotSafe),
}
