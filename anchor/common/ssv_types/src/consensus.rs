use std::{
    collections::HashMap,
    fmt::{Debug, DebugStruct, Display, Formatter},
    hash::Hash,
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
};

use bls::{PublicKeyBytes, Signature};
use derive_more::{From, Into};
use eth2::types::FullBlockContents;
use sha2::{Digest, Sha256};
use slashing_protection::{NotSafe, SlashingDatabase};
use ssz::{Decode, DecodeError, Encode};
use ssz_derive::{Decode, Encode};
use ssz_types::VariableList;
use thiserror::Error;
use tracing::warn;
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use typenum::{
    Pow, Prod, Sum, U2, U3, U4, U5, U11, U13, U23, U56, U64, U131, U308, U700, U852, U1000, U10000,
};
use types::{
    AggregateAndProofBase, AggregateAndProofElectra, AttestationBase, AttestationData,
    AttestationElectra, BlindedBeaconBlock, ChainSpec, Checkpoint, CommitteeIndex, Domain, EthSpec,
    ForkName, Hash256, Slot, SyncCommitteeContribution,
};

use crate::{CommitteeId, ValidatorIndex, message::*, partial_sig::PartialSignatureKind};
//                          UnsignedSSVMessage
//            ----------------------------------------------
//            |                                            |
//            |                                            |
//          SSVMessage                                 FullData
//     ---------------------                          ----------
//     |                   |              ProposerConsensusData/BeaconVote SSZ
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

/// ProposerConsensusData.DataSSZ max size: 8388608 bytes (2^23)
/// This is the maximum size that the proposer consensus data may be
/// Calculated as 2^23 = 8,388,608
pub type ProposerConsensusDataLen = <U2 as Pow<U23>>::Output;

// RoundChange max size: 51852
// This is the maximum size that a round change justification may be
// Calculated as (5 * 10,000) + 1,000 + 852
pub type RoundChangeJustificationLength = Sum<Prod<U5, U10000>, Sum<U1000, U852>>;

// Justification max size: 3700
// This is the maximum size that a prepare justification may be
// Calculated as (3 * 1000) + 700
pub type PrepareJustificationLength = Sum<Prod<U3, U1000>, U700>; // 3700

// AggregatorCommitteeConsensusData max sizes
/// Maximum number of validators per committee that can be aggregators
/// Calculated as 3 * 1000 = 3000
pub type MaxAggregators = Prod<U3, U1000>;
/// Maximum number of sync committee contributors (512 * 4 subnets = 2048)
/// Calculated as 2^11 = 2048
pub type MaxContributors = <U2 as Pow<U11>>::Output;
/// Maximum number of attestation committees
pub type MaxCommitteeIndexes = U64;
/// Number of sync committee subnets (SYNC_COMMITTEE_SUBNET_COUNT)
pub type MaxSyncContributions = U4;
/// Maximum size of an SSZ-encoded aggregated attestation
/// From Go SSV spec: ssz-max:"64,131308" - each attestation up to 131308 bytes
/// Calculated as 131 * 1000 + 308 = 131308
pub type MaxAggregatedAttestationBytes = Sum<Prod<U131, U1000>, U308>;

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
pub struct ProposerConsensusData {
    pub duty: ValidatorDuty,
    pub version: DataVersion,
    pub data_ssz: VariableList<u8, ProposerConsensusDataLen>,
}

impl ProposerConsensusData {
    /// Decode the block data as a blinded beacon block.
    pub fn decode_blinded_block<E: EthSpec>(&self) -> Result<BlindedBeaconBlock<E>, DecodeError> {
        let fork = ForkName::from(self.version);
        BlindedBeaconBlock::from_ssz_bytes_for_fork(&self.data_ssz, fork)
    }

    /// Decode the block data as full block contents (block + blobs).
    pub fn decode_block_contents<E: EthSpec>(&self) -> Result<FullBlockContents<E>, DecodeError> {
        let fork = ForkName::from(self.version);
        FullBlockContents::from_ssz_bytes_for_fork(&self.data_ssz, fork)
    }
}

impl QbftData for ProposerConsensusData {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let bytes = self.as_ssz_bytes();

        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        Hash256::from(hash)
    }
}

pub struct ProposerConsensusDataValidator<E: EthSpec> {
    slashing_database: Arc<SlashingDatabase>,
    disable_slashing_protection: bool,
    spec: Arc<ChainSpec>,
    validator_pubkey: PublicKeyBytes,
    genesis_validators_root: Hash256,
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> QbftDataValidator<ProposerConsensusData> for ProposerConsensusDataValidator<E> {
    fn validate(&self, value: &ProposerConsensusData, our_value: &ProposerConsensusData) -> bool {
        match self.do_validation(value, our_value) {
            Ok(_) => true,
            Err(err) => {
                warn!(%err, "Operator proposed invalid proposer consensus data");
                false
            }
        }
    }
}

impl<E: EthSpec> ProposerConsensusDataValidator<E> {
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
        value: &ProposerConsensusData,
        our_value: &ProposerConsensusData,
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

        // TODO(post-boole): remove `BEACON_ROLE_AGGREGATOR` and
        // `BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION` branches. Post-Boole, `ProposerConsensusData`
        // is only used for `Proposer`.
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
        value: &ProposerConsensusData,
    ) -> Result<(), DataValidationError> {
        // Always do this check, even if we're not validating slashing. This is to ensure that we
        // have a decodable value.
        let header = value
            .decode_blinded_block::<E>()
            .map(|block| block.block_header())
            .or_else(|_| {
                value
                    .decode_block_contents::<E>()
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
    #[error("Unable to decode ssz in ProposerConsensusData: {0:?}")]
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

/// Represents a validator assigned as aggregator with their selection proof.
///
/// Used in `AggregatorCommitteeConsensusData` to track which validators
/// have been selected as aggregators for attestation or sync committee duties.
#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
pub struct AssignedAggregator {
    /// The validator's beacon chain index
    pub validator_index: ValidatorIndex,
    /// The selection proof signature (96 bytes) proving aggregator eligibility
    pub selection_proof: Signature,
    /// Index identifying the duty context. The semantic meaning depends on usage:
    /// - For attestation aggregators: the beacon committee index (0-63)
    /// - For sync committee contributors: the subcommittee index (0-3)
    pub committee_index: u64,
}

/// Consensus data for committee-based aggregator duties.
/// This structure contains all the data needed for committee members to reach consensus
/// on aggregation duties. It supports both attestation aggregation and sync committee
/// contribution aggregation.
#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
pub struct AggregatorCommitteeConsensusData<E: EthSpec> {
    /// Data version (fork) for deserialization of attestations/contributions
    pub version: DataVersion,
    /// Validators selected as attestation aggregators with their selection proofs
    pub aggregators: VariableList<AssignedAggregator, MaxAggregators>,
    /// Committee indexes that have aggregated attestations
    pub aggregator_committee_indexes: VariableList<u64, MaxCommitteeIndexes>,
    /// Aggregated attestations as SSZ bytes, one per committee index
    /// Using bytes because attestation type varies by fork (Base vs Electra)
    pub aggregated_attestations:
        VariableList<VariableList<u8, MaxAggregatedAttestationBytes>, MaxCommitteeIndexes>,
    /// Validators selected as sync committee contributors with their selection proofs
    pub contributors: VariableList<AssignedAggregator, MaxContributors>,
    /// Sync committee contributions, one per subcommittee (4 total)
    pub sync_committee_contributions:
        VariableList<SyncCommitteeContribution<E>, MaxSyncContributions>,
}

impl<E: EthSpec> AggregatorCommitteeConsensusData<E> {
    /// Counts the total number of expected post-consensus signatures for this committee.
    ///
    /// This includes both aggregators and contributors that match the given filter.
    /// Used to ensure all signatures are batched into a single message rather than
    /// being split across multiple messages.
    ///
    /// Returns: count(aggregators) + count(contributors) filtered by committee membership
    pub fn post_consensus_signature_count<F>(&self, is_in_committee: F) -> usize
    where
        F: Fn(&ValidatorIndex) -> bool,
    {
        let aggregator_count = self
            .aggregators
            .iter()
            .filter(|agg| is_in_committee(&agg.validator_index))
            .count();

        let contributor_count = self
            .contributors
            .iter()
            .filter(|contrib| is_in_committee(&contrib.validator_index))
            .count();

        aggregator_count + contributor_count
    }
}

impl<E: EthSpec> QbftData for AggregatorCommitteeConsensusData<E> {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let bytes = self.as_ssz_bytes();
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        Hash256::from(hash)
    }
}

/// Validation errors for AggregatorCommitteeConsensusData
#[derive(Error, Debug)]
pub enum AggregatorCommitteeValidationError {
    #[error(
        "Aggregator committee indexes count ({indexes}) != attestations count ({attestations})"
    )]
    CommitteeIndexCountMismatch { indexes: usize, attestations: usize },
    #[error("Duplicate committee index: {0}")]
    DuplicateCommitteeIndex(u64),
    #[error("Aggregator committee index {0} not in committee indexes list")]
    AggregatorCommitteeIndexMissing(u64),
    #[error("Leftover aggregator committee index not used by any aggregator")]
    AggregatorCommitteeUnusedIndex,
    #[error("Duplicate sync subcommittee index: {0}")]
    DuplicateSyncSubcommittee(u64),
    #[error("Contributor subcommittee {0} not in contributions list")]
    ContributorSubcommitteeMissing(u64),
    #[error("Leftover sync subcommittee index not used by any contributor")]
    SyncSubcommitteeUnusedIndex,
    #[error("No validators assigned")]
    NoValidatorsAssigned,
    #[error("Failed to decode attestation: {0:?}")]
    AttestationDecodeError(ssz::DecodeError),
}

/// Validator for AggregatorCommitteeConsensusData during QBFT consensus.
pub struct AggregatorCommitteeDataValidator<E: EthSpec> {
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> QbftDataValidator<AggregatorCommitteeConsensusData<E>>
    for AggregatorCommitteeDataValidator<E>
{
    fn validate(
        &self,
        value: &AggregatorCommitteeConsensusData<E>,
        _our_value: &AggregatorCommitteeConsensusData<E>,
    ) -> bool {
        match self.do_validation(value) {
            Ok(_) => true,
            Err(err) => {
                warn!(%err, "Operator proposed invalid aggregator committee consensus data");
                false
            }
        }
    }
}

impl<E: EthSpec> Default for AggregatorCommitteeDataValidator<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: EthSpec> AggregatorCommitteeDataValidator<E> {
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }

    /// Ensures the consensus data is internally consistent.
    /// Mirrors ssv-spec validation in https://github.com/ssvlabs/ssv-spec/blob/2e927f79a0fe1da89189735541a6aa2e6069fd5f/types/consensus_data.go#L274-L322
    pub fn do_validation(
        &self,
        value: &AggregatorCommitteeConsensusData<E>,
    ) -> Result<(), AggregatorCommitteeValidationError> {
        // Ensure at least one validator
        if value.aggregators.is_empty() && value.contributors.is_empty() {
            return Err(AggregatorCommitteeValidationError::NoValidatorsAssigned);
        }

        // Aggregators validation

        // Ensure there is exactly one aggregated attestation per committee index
        if value.aggregator_committee_indexes.len() != value.aggregated_attestations.len() {
            return Err(
                AggregatorCommitteeValidationError::CommitteeIndexCountMismatch {
                    indexes: value.aggregator_committee_indexes.len(),
                    attestations: value.aggregated_attestations.len(),
                },
            );
        }

        // Validate equal set (AggregatorsCommitteeIndexes vs. Aggregators.CommitteeIndex)
        for (i, &committee_index) in value.aggregator_committee_indexes.iter().enumerate() {
            // Duplicates are not allowed
            if value.aggregator_committee_indexes[..i].contains(&committee_index) {
                return Err(AggregatorCommitteeValidationError::DuplicateCommitteeIndex(
                    committee_index,
                ));
            }
        }
        let mut used_agg_committees = vec![false; value.aggregator_committee_indexes.len()];
        for agg in value.aggregators.iter() {
            // Check it exists in allowed
            match value
                .aggregator_committee_indexes
                .iter()
                .position(|&ci| ci == agg.committee_index)
            {
                Some(pos) => {
                    // Mark as used
                    used_agg_committees[pos] = true;
                }
                None => {
                    return Err(
                        AggregatorCommitteeValidationError::AggregatorCommitteeIndexMissing(
                            agg.committee_index,
                        ),
                    );
                }
            }
        }
        // Ensure no committee index was left unused (no more than necessary)
        if used_agg_committees.iter().any(|&used| !used) {
            return Err(AggregatorCommitteeValidationError::AggregatorCommitteeUnusedIndex);
        }

        // Ensure attestation objects are decoded correctly
        for att_bytes in value.aggregated_attestations.iter() {
            if value.version >= DataVersion::from(ForkName::Electra) {
                AttestationElectra::<E>::from_ssz_bytes(att_bytes)
                    .map_err(AggregatorCommitteeValidationError::AttestationDecodeError)?;
            } else {
                AttestationBase::<E>::from_ssz_bytes(att_bytes)
                    .map_err(AggregatorCommitteeValidationError::AttestationDecodeError)?;
            }
        }

        // Sync committee contributors validation

        // Validate equal set (`contributors.committee_index` vs
        // `sync_committee_contributions.subcommittee_index`)
        for (i, contrib) in value.sync_committee_contributions.iter().enumerate() {
            // Duplicates are not allowed
            if value.sync_committee_contributions[..i]
                .iter()
                .any(|c| c.subcommittee_index == contrib.subcommittee_index)
            {
                return Err(
                    AggregatorCommitteeValidationError::DuplicateSyncSubcommittee(
                        contrib.subcommittee_index,
                    ),
                );
            }
        }
        let mut used_sc_subnets = vec![false; value.sync_committee_contributions.len()];
        for contributor in value.contributors.iter() {
            // Check it exists in allowed
            match value
                .sync_committee_contributions
                .iter()
                .position(|c| c.subcommittee_index == contributor.committee_index)
            {
                Some(pos) => {
                    // Mark as used
                    used_sc_subnets[pos] = true;
                }
                None => {
                    return Err(
                        AggregatorCommitteeValidationError::ContributorSubcommitteeMissing(
                            contributor.committee_index,
                        ),
                    );
                }
            }
        }
        // Ensure no subcommittee index was left unused (no more than necessary)
        if used_sc_subnets.iter().any(|&used| !used) {
            return Err(AggregatorCommitteeValidationError::SyncSubcommitteeUnusedIndex);
        }

        Ok(())
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
            ForkName::Gloas => 8,
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
            8 => ForkName::Gloas,
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
            ForkName::Gloas => 8,
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
            ForkName::Gloas => 8,
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

#[derive(Clone, Debug, TreeHash, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct PayloadAttestationVote {
    pub beacon_block_root: Hash256,
    pub payload_present: bool,
    pub blob_data_available: bool,
}

impl QbftData for PayloadAttestationVote {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let bytes = self.as_ssz_bytes();

        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        Hash256::from(hash)
    }
}

/// Identifies a batch of pre-consensus selection proofs for a committee.
/// All operators compute the same hash for a given `(slot, committee_id)` pair, ensuring
/// consistent batching across the network.
///
/// Unlike `BeaconVote::hash()` which hashes decided consensus data, this hash is an
/// artificial correlation identifier since pre-consensus selection proofs have different
/// signing roots (attestation vs sync committee selection proofs use different domains).
#[derive(Debug, Clone)]
pub struct SelectionProofBatchId {
    pub slot: Slot,
    pub committee_id: CommitteeId,
}

impl SelectionProofBatchId {
    pub fn new(slot: Slot, committee_id: CommitteeId) -> Self {
        Self { slot, committee_id }
    }

    /// Compute deterministic hash for batching correlation.
    ///
    /// The hash includes the SSZ encoding of `PartialSignatureKind::AggregatorCommitteePartialSig`
    /// as a domain separator to prevent collision with other hashes in the system.
    pub fn hash(&self) -> Hash256 {
        let mut hasher = Sha256::new();
        // Domain separator: SSZ encoding of the partial signature kind
        hasher.update(PartialSignatureKind::AggregatorCommitteePartialSig.as_ssz_bytes());
        hasher.update(self.slot.as_u64().to_le_bytes());
        hasher.update(self.committee_id.0);

        Hash256::from_slice(&hasher.finalize())
    }
}

pub struct BeaconVoteValidator<E: EthSpec> {
    slot: Slot,
    // `None` if slashing protection is disabled via CLI.
    slashing_database: Option<Arc<SlashingDatabase>>,
    spec: Arc<ChainSpec>,
    validator_attestation_committees: HashMap<PublicKeyBytes, u64>,
    genesis_validators_root: Hash256,
    strict_mfp: bool,
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
        slashing_database: Option<Arc<SlashingDatabase>>,
        spec: Arc<ChainSpec>,
        validator_attestation_committees: HashMap<PublicKeyBytes, u64>,
        genesis_validators_root: Hash256,
        strict_mfp: bool,
    ) -> Self {
        Self {
            slot,
            slashing_database,
            spec,
            validator_attestation_committees,
            genesis_validators_root,
            strict_mfp,
            _phantom: PhantomData,
        }
    }

    pub fn do_validation(
        &self,
        value: &BeaconVote,
        our_value: &BeaconVote,
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
        // Exception: At genesis (epoch 0), both source and target are 0 since there's no prior
        // justified checkpoint
        if value.source.epoch >= value.target.epoch
            && (value.source.epoch != 0 || value.target.epoch != 0)
        {
            return Err(BeaconVoteValidationError::TargetNotAfterSource(format!(
                "source {} >= target {}",
                value.source.epoch.as_u64(),
                value.target.epoch.as_u64()
            )));
        }

        // Epoch-only validation (SIP):
        // The target checkpoint refers to the first block of the current epoch on each BN's view.
        // At epoch boundaries (slot 0 of each epoch), BNs are most likely to disagree on that
        // block due to:
        // 1. Network propagation delays at the exact moment of epoch transition
        // 2. The target being the most recent, least-settled block
        // 3. Temporary chain view differences (including benign reorgs)
        //
        // Requiring full checkpoint agreement (epoch AND root) causes systematic liveness issues
        // at the first slot of every epoch, not just during reorgs. This pattern is evidenced by
        // existing special handling for RANDAO signatures at epoch boundaries.
        //
        // Now: Only compare epochs. Root differences are allowed to maintain liveness during
        // epoch transitions and reorgs, while still preventing slashing.
        //
        // Note: This change prioritizes liveness over fork protection. The broader question of
        // whether DVs should actively prevent justifying a potentially wrong fork (vs.
        // focusing solely on slashing protection) remains an open design question. This
        // implementation focuses the SIP on core slashing protection, while cluster-level
        // fork heuristics may be explored separately.
        //
        // See: https://github.com/ssvlabs/ssv-spec/issues/555 (original issue)
        //      https://github.com/ssvlabs/ssv-spec/pull/589 (spec change)
        if self.strict_mfp {
            Self::strict_majority_fork_protection(value, our_value)?;
        } else {
            Self::epoch_majority_fork_protection(value, our_value)?;
        }

        // Check slashing protection for all validator public keys
        self.check_attestation_slashing(value)?;

        Ok(())
    }

    fn epoch_majority_fork_protection(
        value: &BeaconVote,
        our_value: &BeaconVote,
    ) -> Result<(), BeaconVoteValidationError> {
        if value.source.epoch != our_value.source.epoch
            || value.target.epoch != our_value.target.epoch
        {
            Err(BeaconVoteValidationError::EpochMismatch(Box::new(
                EpochMismatch {
                    our_source_epoch: our_value.source.epoch,
                    proposed_source_epoch: value.source.epoch,
                    our_target_epoch: our_value.target.epoch,
                    proposed_target_epoch: value.target.epoch,
                },
            )))
        } else {
            Ok(())
        }
    }

    fn strict_majority_fork_protection(
        value: &BeaconVote,
        our_value: &BeaconVote,
    ) -> Result<(), BeaconVoteValidationError> {
        if value.source != our_value.source || value.target != our_value.target {
            Err(BeaconVoteValidationError::CheckpointMismatch(Box::new(
                CheckpointMismatch {
                    our_source: our_value.source,
                    proposed_source: value.source,
                    our_target: our_value.target,
                    proposed_target: value.target,
                },
            )))
        } else {
            Ok(())
        }
    }

    fn check_attestation_slashing(
        &self,
        value: &BeaconVote,
    ) -> Result<(), BeaconVoteValidationError> {
        let Some(slashing_database) = &self.slashing_database else {
            return Ok(());
        };

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
            slashing_database
                .preliminary_check_attestation(validator_pubkey, &attestation_data, domain_hash)
                .map_err(BeaconVoteValidationError::SlashableAttestation)?;
        }

        Ok(())
    }
}

/// Details about epoch mismatches between our vote and a proposed vote.
///
/// This struct is needed to avoid the linter complaining about the size of the error enum.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointMismatch {
    pub our_source: Checkpoint,
    pub proposed_source: Checkpoint,
    pub our_target: Checkpoint,
    pub proposed_target: Checkpoint,
}

impl Display for CheckpointMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Checkpoint mismatch: SOURCE: our {:?}, proposed {:?}. TARGET: our {:?}, proposed {:?}",
            self.our_source, self.proposed_source, self.our_target, self.proposed_target
        )
    }
}

/// Details about epoch mismatches between our vote and a proposed vote.
///
/// This struct is needed to avoid the linter complaining about the size of the error enum.
#[derive(Debug, Clone, PartialEq)]
pub struct EpochMismatch {
    pub our_source_epoch: types::Epoch,
    pub proposed_source_epoch: types::Epoch,
    pub our_target_epoch: types::Epoch,
    pub proposed_target_epoch: types::Epoch,
}

impl Display for EpochMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Epoch mismatch: SOURCE: our {}, proposed {}. TARGET: our {}, proposed {}",
            self.our_source_epoch,
            self.proposed_source_epoch,
            self.our_target_epoch,
            self.proposed_target_epoch
        )
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
    #[error("{0}")]
    CheckpointMismatch(Box<CheckpointMismatch>),
    #[error("{0}")]
    EpochMismatch(Box<EpochMismatch>),
    #[error("Attestation would be slashable: {0}")]
    SlashableAttestation(NotSafe),
}

#[derive(Error, Debug)]
pub enum PayloadAttestationVoteValidationError {
    #[error("Beacon block root is zero")]
    ZeroBeaconBlockRoot,
}

/// Validator for `PayloadAttestationVote` during QBFT consensus.
///
/// Per SIP-94 SC-2, `payload_present` and `blob_data_available` are trusted
/// from the QBFT leader and intentionally not compared against any local view.
/// The only required check is that `beacon_block_root` is non-zero.
#[derive(Debug, Default)]
pub struct PayloadAttestationVoteValidator;

impl QbftDataValidator<PayloadAttestationVote> for PayloadAttestationVoteValidator {
    fn validate(
        &self,
        value: &PayloadAttestationVote,
        _our_value: &PayloadAttestationVote,
    ) -> bool {
        match self.do_validation(value) {
            Ok(_) => true,
            Err(err) => {
                warn!(%err, "Operator proposed invalid payload attestation vote");
                false
            }
        }
    }
}

impl PayloadAttestationVoteValidator {
    pub fn do_validation(
        &self,
        value: &PayloadAttestationVote,
    ) -> Result<(), PayloadAttestationVoteValidationError> {
        if value.beacon_block_root.is_zero() {
            return Err(PayloadAttestationVoteValidationError::ZeroBeaconBlockRoot);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use bls::{AggregateSignature, FixedBytesExtended};
    use ssz_types::{BitList, BitVector};
    use types::{Checkpoint, Epoch, MainnetEthSpec, SyncCommitteeContribution};

    use super::*;

    // ═══════════════════════════════════════════════════════════════════════════════
    // AssignedAggregator Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Creates a test AssignedAggregator with specified values
    fn create_assigned_aggregator(
        validator_index: usize,
        committee_index: u64,
    ) -> AssignedAggregator {
        AssignedAggregator {
            validator_index: ValidatorIndex(validator_index),
            selection_proof: Signature::empty(),
            committee_index,
        }
    }

    #[test]
    fn assigned_aggregator_ssz_roundtrip() {
        let aggregator = create_assigned_aggregator(12345, 42);

        let encoded = aggregator.as_ssz_bytes();
        let decoded = AssignedAggregator::from_ssz_bytes(&encoded).unwrap();

        assert_eq!(aggregator, decoded);
    }

    #[test]
    fn assigned_aggregator_ssz_byte_layout() {
        // Verify wire format matches expected layout:
        // - validator_index: bytes 0-7 (u64 little-endian)
        // - selection_proof: bytes 8-103 (96 bytes)
        // - committee_index: bytes 104-111 (u64 little-endian)
        let aggregator = create_assigned_aggregator(0x0102030405060708, 0x1112131415161718);

        let encoded = aggregator.as_ssz_bytes();

        // Check total size
        assert_eq!(encoded.len(), 112, "AssignedAggregator should be 112 bytes");

        // Check validator_index at bytes 0-7 (little-endian)
        assert_eq!(
            &encoded[0..8],
            &[0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]
        );

        // Check committee_index at bytes 104-111 (little-endian)
        assert_eq!(
            &encoded[104..112],
            &[0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11]
        );
    }

    #[test]
    fn assigned_aggregator_decode_invalid_length() {
        // Too short
        let short_bytes = vec![0u8; 50];
        assert!(AssignedAggregator::from_ssz_bytes(&short_bytes).is_err());

        // Too long
        let long_bytes = vec![0u8; 200];
        assert!(AssignedAggregator::from_ssz_bytes(&long_bytes).is_err());
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // AggregatorCommitteeConsensusData Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Creates an empty AggregatorCommitteeConsensusData for testing
    fn create_empty_consensus_data() -> AggregatorCommitteeConsensusData<MainnetEthSpec> {
        AggregatorCommitteeConsensusData {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::empty(),
            aggregator_committee_indexes: VariableList::empty(),
            aggregated_attestations: VariableList::empty(),
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        }
    }

    /// Helper to create valid attestation bytes for testing (pre-Electra format)
    fn create_test_attestation_bytes(
        index: u64,
    ) -> VariableList<u8, MaxAggregatedAttestationBytes> {
        let attestation = AttestationBase::<MainnetEthSpec> {
            aggregation_bits: BitList::with_capacity(128).unwrap(),
            data: AttestationData {
                slot: Slot::new(1000),
                index,
                beacon_block_root: Hash256::zero(),
                source: Checkpoint {
                    epoch: Epoch::new(10),
                    root: Hash256::zero(),
                },
                target: Checkpoint {
                    epoch: Epoch::new(11),
                    root: Hash256::zero(),
                },
            },
            signature: AggregateSignature::infinity(),
        };
        VariableList::new(attestation.as_ssz_bytes()).unwrap()
    }

    /// Creates a populated AggregatorCommitteeConsensusData for testing
    fn create_populated_consensus_data() -> AggregatorCommitteeConsensusData<MainnetEthSpec> {
        // Create aggregators for committee index 5
        let aggregators = vec![create_assigned_aggregator(100, 5)];

        AggregatorCommitteeConsensusData {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::new(aggregators).unwrap(),
            aggregator_committee_indexes: VariableList::new(vec![5]).unwrap(),
            aggregated_attestations: VariableList::new(vec![create_test_attestation_bytes(5)])
                .unwrap(),
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        }
    }

    #[test]
    fn aggregator_committee_consensus_data_empty_roundtrip() {
        let data = create_empty_consensus_data();

        let encoded = data.as_ssz_bytes();
        let decoded =
            AggregatorCommitteeConsensusData::<MainnetEthSpec>::from_ssz_bytes(&encoded).unwrap();

        assert_eq!(data, decoded);
    }

    #[test]
    fn aggregator_committee_consensus_data_populated_roundtrip() {
        let data = create_populated_consensus_data();

        let encoded = data.as_ssz_bytes();
        let decoded =
            AggregatorCommitteeConsensusData::<MainnetEthSpec>::from_ssz_bytes(&encoded).unwrap();

        assert_eq!(data, decoded);
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // AggregatorCommitteeDataValidator Tests - Rejection Cases
    // ═══════════════════════════════════════════════════════════════════════════════

    fn create_aggregator_committee_validator() -> AggregatorCommitteeDataValidator<MainnetEthSpec> {
        AggregatorCommitteeDataValidator::new()
    }

    /// Helper to create a valid sync committee contribution
    fn create_sync_contribution(
        subcommittee_index: u64,
    ) -> SyncCommitteeContribution<MainnetEthSpec> {
        SyncCommitteeContribution {
            slot: Slot::new(1000),
            beacon_block_root: Hash256::zero(),
            subcommittee_index,
            aggregation_bits: BitVector::default(),
            signature: AggregateSignature::infinity(),
        }
    }

    /// Helper to create valid attestation bytes for a committee index (for validator tests)
    fn create_attestation_bytes(index: u64) -> VariableList<u8, MaxAggregatedAttestationBytes> {
        create_test_attestation_bytes(index)
    }

    #[test]
    fn validator_rejects_no_validators_assigned() {
        let validator = create_aggregator_committee_validator();
        let data = create_empty_consensus_data();

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(AggregatorCommitteeValidationError::NoValidatorsAssigned)
        ));
    }

    #[test]
    fn validator_rejects_committee_index_count_mismatch() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::new(vec![create_assigned_aggregator(100, 5)]).unwrap(),
            aggregator_committee_indexes: VariableList::new(vec![5, 10]).unwrap(), // 2 indexes
            aggregated_attestations: VariableList::new(vec![create_attestation_bytes(5)]).unwrap(), /* 1 attestation */
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        };

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(
                AggregatorCommitteeValidationError::CommitteeIndexCountMismatch {
                    indexes: 2,
                    attestations: 1
                }
            )
        ));
    }

    #[test]
    fn validator_rejects_duplicate_committee_index() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::new(vec![
                create_assigned_aggregator(100, 5),
                create_assigned_aggregator(101, 5),
            ])
            .unwrap(),
            aggregator_committee_indexes: VariableList::new(vec![5, 5]).unwrap(), // Duplicate!
            aggregated_attestations: VariableList::new(vec![
                create_attestation_bytes(5),
                create_attestation_bytes(5),
            ])
            .unwrap(),
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        };

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(AggregatorCommitteeValidationError::DuplicateCommitteeIndex(
                5
            ))
        ));
    }

    #[test]
    fn validator_rejects_aggregator_missing_committee_index() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::new(vec![
                create_assigned_aggregator(100, 5),
                create_assigned_aggregator(101, 99), // References index 99 which doesn't exist
            ])
            .unwrap(),
            aggregator_committee_indexes: VariableList::new(vec![5]).unwrap(),
            aggregated_attestations: VariableList::new(vec![create_attestation_bytes(5)]).unwrap(),
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        };

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(AggregatorCommitteeValidationError::AggregatorCommitteeIndexMissing(99))
        ));
    }

    #[test]
    fn validator_rejects_unused_committee_index() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::new(vec![create_assigned_aggregator(100, 5)]).unwrap(), /* Only uses index 5 */
            aggregator_committee_indexes: VariableList::new(vec![5, 10]).unwrap(), /* Has unused
                                                                                    * index 10 */
            aggregated_attestations: VariableList::new(vec![
                create_attestation_bytes(5),
                create_attestation_bytes(10),
            ])
            .unwrap(),
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        };

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(AggregatorCommitteeValidationError::AggregatorCommitteeUnusedIndex)
        ));
    }

    #[test]
    fn validator_rejects_duplicate_sync_subcommittee() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::empty(),
            aggregator_committee_indexes: VariableList::empty(),
            aggregated_attestations: VariableList::empty(),
            contributors: VariableList::new(vec![
                create_assigned_aggregator(100, 0),
                create_assigned_aggregator(101, 0),
            ])
            .unwrap(),
            sync_committee_contributions: VariableList::new(vec![
                create_sync_contribution(0),
                create_sync_contribution(0), // Duplicate subcommittee!
            ])
            .unwrap(),
        };

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(AggregatorCommitteeValidationError::DuplicateSyncSubcommittee(0))
        ));
    }

    #[test]
    fn validator_rejects_contributor_missing_subcommittee() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::empty(),
            aggregator_committee_indexes: VariableList::empty(),
            aggregated_attestations: VariableList::empty(),
            contributors: VariableList::new(vec![
                create_assigned_aggregator(100, 0),
                create_assigned_aggregator(101, 3), /* References subcommittee 3 which doesn't
                                                     * exist */
            ])
            .unwrap(),
            sync_committee_contributions: VariableList::new(vec![create_sync_contribution(0)])
                .unwrap(),
        };

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(AggregatorCommitteeValidationError::ContributorSubcommitteeMissing(3))
        ));
    }

    #[test]
    fn validator_rejects_unused_sync_subcommittee() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::empty(),
            aggregator_committee_indexes: VariableList::empty(),
            aggregated_attestations: VariableList::empty(),
            contributors: VariableList::new(vec![create_assigned_aggregator(100, 0)]).unwrap(), /* Only uses subcommittee 0 */
            sync_committee_contributions: VariableList::new(vec![
                create_sync_contribution(0),
                create_sync_contribution(1), // Unused subcommittee 1
            ])
            .unwrap(),
        };

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(AggregatorCommitteeValidationError::SyncSubcommitteeUnusedIndex)
        ));
    }

    #[test]
    fn validator_rejects_invalid_attestation_bytes() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::new(vec![create_assigned_aggregator(100, 5)]).unwrap(),
            aggregator_committee_indexes: VariableList::new(vec![5]).unwrap(),
            aggregated_attestations: VariableList::new(vec![
                VariableList::new(vec![0u8; 10]).unwrap(), // Invalid attestation bytes
            ])
            .unwrap(),
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        };

        let result = validator.do_validation(&data);
        assert!(matches!(
            result,
            Err(AggregatorCommitteeValidationError::AttestationDecodeError(
                _
            ))
        ));
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // AggregatorCommitteeDataValidator Tests - Acceptance Cases
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn validator_accepts_valid_aggregators_only() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::new(vec![
                create_assigned_aggregator(100, 5),
                create_assigned_aggregator(101, 5),
                create_assigned_aggregator(200, 10),
            ])
            .unwrap(),
            aggregator_committee_indexes: VariableList::new(vec![5, 10]).unwrap(),
            aggregated_attestations: VariableList::new(vec![
                create_attestation_bytes(5),
                create_attestation_bytes(10),
            ])
            .unwrap(),
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        };

        let result = validator.do_validation(&data);
        assert!(result.is_ok(), "Expected validation to pass: {:?}", result);
    }

    #[test]
    fn validator_accepts_valid_contributors_only() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::empty(),
            aggregator_committee_indexes: VariableList::empty(),
            aggregated_attestations: VariableList::empty(),
            contributors: VariableList::new(vec![
                create_assigned_aggregator(100, 0),
                create_assigned_aggregator(101, 0),
                create_assigned_aggregator(200, 1),
                create_assigned_aggregator(201, 2),
            ])
            .unwrap(),
            sync_committee_contributions: VariableList::new(vec![
                create_sync_contribution(0),
                create_sync_contribution(1),
                create_sync_contribution(2),
            ])
            .unwrap(),
        };

        let result = validator.do_validation(&data);
        assert!(result.is_ok(), "Expected validation to pass: {:?}", result);
    }

    #[test]
    fn validator_accepts_valid_both_aggregators_and_contributors() {
        let validator = create_aggregator_committee_validator();
        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: VariableList::new(vec![
                create_assigned_aggregator(100, 5),
                create_assigned_aggregator(101, 10),
            ])
            .unwrap(),
            aggregator_committee_indexes: VariableList::new(vec![5, 10]).unwrap(),
            aggregated_attestations: VariableList::new(vec![
                create_attestation_bytes(5),
                create_attestation_bytes(10),
            ])
            .unwrap(),
            contributors: VariableList::new(vec![
                create_assigned_aggregator(200, 0),
                create_assigned_aggregator(201, 1),
            ])
            .unwrap(),
            sync_committee_contributions: VariableList::new(vec![
                create_sync_contribution(0),
                create_sync_contribution(1),
            ])
            .unwrap(),
        };

        let result = validator.do_validation(&data);
        assert!(result.is_ok(), "Expected validation to pass: {:?}", result);
    }

    #[test]
    fn validator_accepts_electra_attestations() {
        let validator = create_aggregator_committee_validator();

        // Create an Electra attestation
        let attestation = AttestationElectra::<MainnetEthSpec> {
            aggregation_bits: BitList::with_capacity(128).unwrap(),
            data: AttestationData {
                slot: Slot::new(1000),
                index: 0, // Electra uses committee_bits instead
                beacon_block_root: Hash256::zero(),
                source: Checkpoint {
                    epoch: Epoch::new(10),
                    root: Hash256::zero(),
                },
                target: Checkpoint {
                    epoch: Epoch::new(11),
                    root: Hash256::zero(),
                },
            },
            signature: AggregateSignature::infinity(),
            committee_bits: BitVector::default(),
        };

        let data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Electra),
            aggregators: VariableList::new(vec![create_assigned_aggregator(100, 5)]).unwrap(),
            aggregator_committee_indexes: VariableList::new(vec![5]).unwrap(),
            aggregated_attestations: VariableList::new(vec![
                VariableList::new(attestation.as_ssz_bytes()).unwrap(),
            ])
            .unwrap(),
            contributors: VariableList::empty(),
            sync_committee_contributions: VariableList::empty(),
        };

        let result = validator.do_validation(&data);
        assert!(
            result.is_ok(),
            "Expected Electra attestation validation to pass: {:?}",
            result
        );
    }

    #[test]
    fn validator_via_qbft_trait() {
        // Test using the QbftDataValidator trait interface
        let validator = create_aggregator_committee_validator();
        let valid_data = create_populated_consensus_data();
        let our_data = create_populated_consensus_data();

        // QbftDataValidator::validate should return true for valid data
        assert!(
            QbftDataValidator::validate(&validator, &valid_data, &our_data),
            "QbftDataValidator trait should accept valid data"
        );

        // QbftDataValidator::validate should return false for invalid data
        let invalid_data = create_empty_consensus_data();
        assert!(
            !QbftDataValidator::validate(&validator, &invalid_data, &our_data),
            "QbftDataValidator trait should reject invalid data"
        );
    }

    /// Helper function to create a BeaconVoteValidator for testing.
    /// This validator has slashing protection disabled for simpler testing.
    fn create_test_validator(strict_mfp: bool) -> BeaconVoteValidator<MainnetEthSpec> {
        let spec = Arc::new(ChainSpec::mainnet());
        let validator_attestation_committees = HashMap::new();
        let genesis_validators_root = Hash256::zero();
        let slot = Slot::new(100);

        BeaconVoteValidator::new(
            slot,
            None,
            spec,
            validator_attestation_committees,
            genesis_validators_root,
            strict_mfp,
        )
    }

    #[test]
    fn test_mismatched_source_different_epochs() {
        let validator = create_test_validator(false);

        let our_source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = BeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
        };

        // Create a proposed vote with different source epoch
        let proposed_source = Checkpoint {
            epoch: Epoch::new(1), // Different epoch
            root: Hash256::from_low_u64_be(1),
        };
        let proposed_vote = BeaconVote {
            block_root: Hash256::random(),
            source: proposed_source,
            target: our_target,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(result.is_err());
        match result.unwrap_err() {
            BeaconVoteValidationError::EpochMismatch(mismatch) => {
                assert_eq!(mismatch.our_source_epoch, our_source.epoch);
                assert_eq!(mismatch.proposed_source_epoch, proposed_source.epoch);
                assert_eq!(mismatch.our_target_epoch, our_target.epoch);
                assert_eq!(mismatch.proposed_target_epoch, our_target.epoch);
            }
            err => panic!("Expected EpochMismatch error, got: {:?}", err),
        }
    }

    #[test]
    fn test_valid_source_equals_target_at_epoch_zero() {
        let validator = create_test_validator(false);

        let our_source = Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::from_low_u64_be(1),
        };
        let our_vote = BeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
        };

        let proposed_target = Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::from_low_u64_be(2),
        };
        let proposed_vote = BeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: proposed_target,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mismatched_target_different_epochs() {
        let validator = create_test_validator(false);

        let our_source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = BeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
        };

        // Create a proposed vote with different target epoch
        let proposed_target = Checkpoint {
            epoch: Epoch::new(4), // Different epoch (but still valid, current epoch is 3, max is 4)
            root: Hash256::from_low_u64_be(2),
        };
        let proposed_vote = BeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: proposed_target,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(result.is_err());
        match result.unwrap_err() {
            BeaconVoteValidationError::EpochMismatch(mismatch) => {
                assert_eq!(mismatch.our_source_epoch, our_source.epoch);
                assert_eq!(mismatch.proposed_source_epoch, our_source.epoch);
                assert_eq!(mismatch.our_target_epoch, our_target.epoch);
                assert_eq!(mismatch.proposed_target_epoch, proposed_target.epoch);
            }
            err => panic!("Expected EpochMismatch error, got: {:?}", err),
        }
    }

    #[test]
    fn test_valid_matching_checkpoints() {
        for strict_mfp in [true, false] {
            let validator = create_test_validator(strict_mfp);

            let source = Checkpoint {
                epoch: Epoch::new(2),
                root: Hash256::from_low_u64_be(1),
            };
            let target = Checkpoint {
                epoch: Epoch::new(3),
                root: Hash256::from_low_u64_be(2),
            };

            let our_vote = BeaconVote {
                block_root: Hash256::random(),
                source,
                target,
            };
            // Proposed vote has same source and target (but different head vote)
            let proposed_vote = BeaconVote {
                block_root: Hash256::random(),
                source,
                target,
            };

            // This should succeed since epochs match
            let result = validator.do_validation(&proposed_vote, &our_vote);
            assert!(
                result.is_ok(),
                "Expected validation to succeed for matching epochs, got error: {:?}",
                result.unwrap_err()
            );
        }
    }

    #[test]
    fn test_valid_matching_epochs_different_roots() {
        let validator = create_test_validator(false);

        let our_source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = BeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
        };

        // Proposed vote has same epochs but different roots (simulating reorg)
        let proposed_source = Checkpoint {
            epoch: Epoch::new(2),                // Same epoch
            root: Hash256::from_low_u64_be(999), // Different root
        };
        let proposed_target = Checkpoint {
            epoch: Epoch::new(3),                // Same epoch
            root: Hash256::from_low_u64_be(888), // Different root
        };
        let proposed_vote = BeaconVote {
            block_root: Hash256::random(),
            source: proposed_source,
            target: proposed_target,
        };

        // This should succeed since epochs match (roots don't need to match)
        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(
            result.is_ok(),
            "Expected validation to succeed for matching epochs with different roots, got error: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn test_strict_mismatched_target_different_roots() {
        let validator = create_test_validator(true);

        let our_source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = BeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
        };

        // Create a proposed vote with same target epoch but different root
        let proposed_target = Checkpoint {
            epoch: Epoch::new(3),                // Same epoch
            root: Hash256::from_low_u64_be(999), // Different root
        };
        let proposed_vote = BeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: proposed_target,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(result.is_err());
        match result.unwrap_err() {
            BeaconVoteValidationError::CheckpointMismatch(mismatch) => {
                assert_eq!(mismatch.our_source, our_source);
                assert_eq!(mismatch.proposed_source, our_source);
                assert_eq!(mismatch.our_target, our_target);
                assert_eq!(mismatch.proposed_target, proposed_target);
            }
            err => panic!("Expected DifferentCheckpoint error, got: {:?}", err),
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // SelectionProofBatchId Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_selection_proof_batch_id_same_inputs_same_hash() {
        let slot = Slot::new(12345);
        let committee_id = CommitteeId::from([0u8; 32]);

        // Compute hash twice with same inputs
        let batch_id1 = SelectionProofBatchId::new(slot, committee_id);
        let batch_id2 = SelectionProofBatchId::new(slot, committee_id);

        // Same inputs should produce same hash
        assert_eq!(batch_id1.hash(), batch_id2.hash());
    }

    #[test]
    fn test_selection_proof_batch_id_different_slots() {
        let committee_id = CommitteeId::from([1u8; 32]);

        // Different slots
        let slot1 = Slot::new(12345);
        let slot2 = Slot::new(12346);

        let batch_id1 = SelectionProofBatchId::new(slot1, committee_id);
        let batch_id2 = SelectionProofBatchId::new(slot2, committee_id);

        // Different slots should produce different hashes
        assert_ne!(batch_id1.hash(), batch_id2.hash());
    }

    #[test]
    fn test_selection_proof_batch_id_different_committees() {
        let slot = Slot::new(12345);

        // Different committee IDs
        let committee_id1 = CommitteeId::from([1u8; 32]);
        let committee_id2 = CommitteeId::from([2u8; 32]);

        let batch_id1 = SelectionProofBatchId::new(slot, committee_id1);
        let batch_id2 = SelectionProofBatchId::new(slot, committee_id2);

        // Different committees should produce different hashes
        assert_ne!(batch_id1.hash(), batch_id2.hash());
    }

    #[test]
    fn test_selection_proof_batch_id_deterministic_across_operators() {
        // This test verifies that the hash is deterministic and identical
        // across different operators for the same inputs

        let slot = Slot::new(42);
        let committee_bytes = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20, 0x30, 0x40,
            0x50, 0x60, 0x70, 0x80,
        ];
        let committee_id = CommitteeId::from(committee_bytes);

        let batch_id = SelectionProofBatchId::new(slot, committee_id);
        let expected_hash = batch_id.hash();

        // Simulate computing on different "operators" (same calculation repeated)
        for _ in 0..3 {
            let batch_id = SelectionProofBatchId::new(slot, committee_id);
            // All operators should get the same hash
            assert_eq!(batch_id.hash(), expected_hash);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // PayloadAttestationVote Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    fn create_payload_attestation_vote(
        root: Hash256,
        payload_present: bool,
        blob_data_available: bool,
    ) -> PayloadAttestationVote {
        PayloadAttestationVote {
            beacon_block_root: root,
            payload_present,
            blob_data_available,
        }
    }

    #[test]
    fn test_payload_attestation_vote_ssz_roundtrip() {
        for (payload_present, blob_data_available) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let vote = create_payload_attestation_vote(
                Hash256::from_low_u64_be(0xabcd),
                payload_present,
                blob_data_available,
            );

            let encoded = vote.as_ssz_bytes();
            let decoded = PayloadAttestationVote::from_ssz_bytes(&encoded).unwrap();

            assert_eq!(vote, decoded);
        }
    }

    #[test]
    fn test_payload_attestation_vote_ssz_byte_layout() {
        // Fixed-length container: 32-byte root + 1-byte bool + 1-byte bool = 34 bytes.
        let root_bytes = [0xAA; 32];
        let vote = create_payload_attestation_vote(Hash256::from(root_bytes), true, false);

        let encoded = vote.as_ssz_bytes();

        assert_eq!(
            encoded.len(),
            34,
            "PayloadAttestationVote should encode to 34 bytes"
        );
        assert_eq!(&encoded[0..32], &root_bytes);
        assert_eq!(encoded[32], 1, "payload_present byte");
        assert_eq!(encoded[33], 0, "blob_data_available byte");
    }

    #[test]
    fn test_payload_attestation_vote_hash_deterministic() {
        let root = Hash256::from_low_u64_be(0x1234);
        let vote = create_payload_attestation_vote(root, true, false);

        // Hashing two independently-constructed values with the same inputs
        // must produce the same hash (catches identity- or address-dependent hashing).
        let same = create_payload_attestation_vote(root, true, false);
        assert_eq!(vote.hash(), same.hash());

        let different_root =
            create_payload_attestation_vote(Hash256::from_low_u64_be(0x5678), true, false);
        assert_ne!(vote.hash(), different_root.hash());

        let flipped_payload = create_payload_attestation_vote(root, false, false);
        assert_ne!(vote.hash(), flipped_payload.hash());

        let flipped_blob = create_payload_attestation_vote(root, true, true);
        assert_ne!(vote.hash(), flipped_blob.hash());
    }

    #[test]
    fn test_payload_attestation_vote_validator_rejects_zero_root() {
        let validator = PayloadAttestationVoteValidator;
        let zero_vote = create_payload_attestation_vote(Hash256::zero(), true, true);

        let result = validator.do_validation(&zero_vote);
        assert!(matches!(
            result,
            Err(PayloadAttestationVoteValidationError::ZeroBeaconBlockRoot)
        ));
    }

    #[test]
    fn test_payload_attestation_vote_validator_accepts_nonzero_root() {
        let validator = PayloadAttestationVoteValidator;
        let root = Hash256::from_low_u64_be(0xdead);

        for (payload_present, blob_data_available) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let vote = create_payload_attestation_vote(root, payload_present, blob_data_available);
            assert!(
                validator.do_validation(&vote).is_ok(),
                "validator should accept non-zero root regardless of payload-status flags \
                 (payload_present={payload_present}, blob_data_available={blob_data_available})"
            );
        }
    }

    #[test]
    fn test_payload_attestation_vote_validator_ignores_start_value() {
        // SIP-94 SC-2: payload-status fields are trusted from the QBFT leader;
        // the validator must NOT compare the proposed value against the local
        // operator's view (start_value). Verified at the trait-surface level by
        // calling `validate(value, our_value)` with divergent values and asserting
        // acceptance; structurally also guaranteed by `do_validation` only taking
        // `value`.
        let validator = PayloadAttestationVoteValidator;
        let proposed =
            create_payload_attestation_vote(Hash256::from_low_u64_be(0x1111), true, true);
        let local_view =
            create_payload_attestation_vote(Hash256::from_low_u64_be(0x2222), false, false);

        assert!(validator.validate(&proposed, &local_view));
    }
}
