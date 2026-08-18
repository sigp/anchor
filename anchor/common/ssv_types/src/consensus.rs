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
    AggregateAndProof, AggregateAndProofBase, AggregateAndProofElectra, AggregateAndProofGloas,
    Attestation, AttestationBase, AttestationData, AttestationElectra, AttestationGloas,
    BeaconBlock, BlindedBeaconBlock, ChainSpec, Checkpoint, CommitteeIndex, Domain, EthSpec,
    ExecutionPayloadEnvelope, ExecutionRequestsGloas, ForkName, Hash256, SignedRoot, Slot,
    SyncCommitteeContribution, consts::gloas::BUILDER_INDEX_SELF_BUILD,
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
        if fork >= ForkName::Gloas {
            return Err(DecodeError::NoMatchingVariant);
        }
        BlindedBeaconBlock::from_ssz_bytes_for_fork(&self.data_ssz, fork)
    }

    /// Decode the block data as full block contents (block + blobs).
    pub fn decode_block_contents<E: EthSpec>(&self) -> Result<FullBlockContents<E>, DecodeError> {
        let fork = ForkName::from(self.version);
        FullBlockContents::from_ssz_bytes_for_fork(&self.data_ssz, fork)
    }

    /// Decode as a full beacon block shape.
    pub fn decode_block<E: EthSpec>(&self) -> Result<BeaconBlock<E>, DecodeError> {
        let fork = ForkName::from(self.version);
        BeaconBlock::from_ssz_bytes_for_fork(&self.data_ssz, fork)
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

/// QBFT consensus value for execution payload envelope signing duty (SIP-94 §6, EIP-7732 ePBS).
///
/// Wire-identical to `ProposerConsensusData` but a distinct type to enable routing
/// discrimination in `QbftDecidable` implementations. Under ePBS, envelope-signing
/// consensus uses this type, while beacon block proposal continues to use
/// `ProposerConsensusData`. The opaque `data_ssz` bytes contain a
/// `BlindedExecutionPayloadEnvelope<E>` (via `decode_blinded_envelope`).
#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
pub struct EnvelopeConsensusData {
    pub duty: ValidatorDuty,
    pub version: DataVersion,
    pub data_ssz: VariableList<u8, ProposerConsensusDataLen>,
}

impl EnvelopeConsensusData {
    /// Decode `data_ssz` as a `BlindedExecutionPayloadEnvelope`.
    pub fn decode_blinded_envelope<E: EthSpec>(
        &self,
    ) -> Result<BlindedExecutionPayloadEnvelope<E>, DecodeError> {
        BlindedExecutionPayloadEnvelope::<E>::from_ssz_bytes(&self.data_ssz)
    }
}

impl QbftData for EnvelopeConsensusData {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        let bytes = self.as_ssz_bytes();

        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        Hash256::from(hash)
    }
}

/// Blinded envelope for QBFT consensus over execution payload proposals (SIP-94 §6).
///
/// Under ePBS (EIP-7732), the cluster runs consensus over this blinded form rather than the
/// full multi-MB `ExecutionPayloadEnvelope`. The proposer substitutes the full `payload` field
/// with its tree-hash root, preserving SSZ merkleization parity: the blinded envelope's root
/// equals the full envelope's root, so a signature over the blinded signing root is valid for
/// the full envelope.
///
/// EIP-7688 makes the full `ExecutionPayloadEnvelope` a progressive container, so this mirror
/// must merkleize progressively too or the parity above breaks (SIP-94 §6).
#[derive(Clone, Debug, PartialEq, Encode, Decode, TreeHash)]
#[tree_hash(
    struct_behaviour = "progressive_container",
    active_fields(1, 1, 1, 1, 1)
)]
pub struct BlindedExecutionPayloadEnvelope<E: EthSpec> {
    /// Tree-hash root of the full `payload` (`ExecutionPayloadGloas`).
    pub payload_root: Hash256,
    /// Execution requests (deposits, withdrawals, consolidations, plus Gloas-added
    /// `builder_deposits` and `builder_exits`), copied verbatim from the full envelope.
    pub execution_requests: ExecutionRequestsGloas<E>,
    /// Builder index: `u64::MAX` (self-build) or the builder's registry index for external
    /// builds. Used to attribute the payload source.
    pub builder_index: u64,
    /// Root of the beacon block that this envelope is proposed within. Used to bind the
    /// envelope to the proposing beacon block.
    pub beacon_block_root: Hash256,
    /// Root of the parent beacon block, used for fork-choice context.
    pub parent_beacon_block_root: Hash256,
}

impl<E: EthSpec> SignedRoot for BlindedExecutionPayloadEnvelope<E> {}

impl<E: EthSpec> BlindedExecutionPayloadEnvelope<E> {
    /// Build the blinded envelope from the full `ExecutionPayloadEnvelope`, substituting the
    /// `payload` field with its tree-hash root.
    pub fn from_full(full: &ExecutionPayloadEnvelope<E>) -> Self {
        Self {
            payload_root: full.payload.tree_hash_root(),
            execution_requests: full.execution_requests.clone(),
            builder_index: full.builder_index,
            beacon_block_root: full.beacon_block_root,
            parent_beacon_block_root: full.parent_beacon_block_root,
        }
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
                // SIP-94 §2: `version` is leader-supplied and selects the decode shape (and
                // thus the signing-root merkleization), so bind it to our own candidate before
                // decoding. The proposer branch pins `version` to the duty-slot fork in
                // `validate_block_proposal` instead.
                if value.version != our_value.version {
                    return Err(DataValidationError::VersionMismatch {
                        expected: ForkName::from(our_value.version),
                        got: ForkName::from(value.version),
                    });
                }
                value
                    .version
                    .decode_aggregate_and_proof::<E>(&value.data_ssz)
                    .map_err(DataValidationError::ForkDecode)?;
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
        // SIP-94 §4: `version` is leader-supplied and selects how `data_ssz` is decoded, so it
        // must equal the fork scheduled at the duty slot (`value.duty.slot` is pinned to our own
        // duty by the `SlotMismatch` check in `do_validation`).
        let expected = self.spec.fork_name_at_slot::<E>(value.duty.slot);
        let fork = ForkName::from(value.version);
        if fork != expected {
            return Err(DataValidationError::VersionMismatch {
                expected,
                got: fork,
            });
        }

        // Decode the block header to ensure the value is decodable (even when slashing
        // protection is disabled). Under Gloas (EIP-7732), DataSSZ is decoded directly as a plain
        // BeaconBlock. This behaviour is not behaviourally load-bearing as the execution
        // payload is decoupled from the block body. The outcome of `decode_blinded_block`
        // and `decode_block` are identical for this variant. The Pre-Gloas branch
        // preserves the existing try-blinded-then-full fallback.
        let header = if fork >= ForkName::Gloas {
            value
                .decode_block::<E>()
                .map(|block| block.block_header())
                .map_err(DataValidationError::DecodeError)?
        } else {
            value
                .decode_blinded_block::<E>()
                .map(|block| block.block_header())
                .or_else(|_| {
                    value
                        .decode_block_contents::<E>()
                        .map(|block| block.block().block_header())
                })
                .map_err(DataValidationError::DecodeError)?
        };

        // The block is signed under `header.slot` and slashing protection keys on it, so when
        // the slashing DB has no conflicting entry at that slot (or protection is disabled),
        // nothing ties the signed slot back to the duty: a leader embedding a block built for a
        // different slot could harvest a signature for it. Pin it (go-ssv applies the same pin
        // on its Gloas path). Also bounds `header.slot` for the epoch/domain derivation below.
        if header.slot != value.duty.slot {
            return Err(DataValidationError::BlockSlotMismatch {
                expected: value.duty.slot,
                got: header.slot,
            });
        }

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
    #[error("Unable to decode consensus payload: {0}")]
    ForkDecode(ForkDecodeError),
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
    #[error("wrong data version: expected fork {expected:?}, got {got:?}")]
    VersionMismatch { expected: ForkName, got: ForkName },
    #[error("wrong block slot: expected {expected}, got {got}")]
    BlockSlotMismatch { expected: Slot, got: Slot },
    #[error("Block proposal would be slashable: {0}")]
    SlashableBlockProposal(NotSafe),
}

impl From<DecodeError> for DataValidationError {
    fn from(err: DecodeError) -> Self {
        DataValidationError::DecodeError(err)
    }
}

/// Validation errors for `EnvelopeConsensusData` during envelope-signing QBFT.
#[derive(Error, Debug)]
pub enum EnvelopeValidationError {
    #[error("Unable to decode ssz in EnvelopeConsensusData: {0:?}")]
    DecodeError(DecodeError),
    #[error("Slot mismatch: expected {expected}, got {got}")]
    SlotMismatch { expected: Slot, got: Slot },
    #[error("wrong validator index: expected {expected:?}, got {got:?}")]
    IndexMismatch {
        expected: ValidatorIndex,
        got: ValidatorIndex,
    },
    #[error("wrong validator pk: expected {expected:?}, got {got:?}")]
    PubKeyMismatch {
        expected: PublicKeyBytes,
        got: PublicKeyBytes,
    },
    #[error("envelope builder_index is not self-build: {0}")]
    NotSelfBuild(u64),
    #[error("beacon_block_root mismatch: expected {expected:?}, got {got:?}")]
    DecidedRootMismatch { expected: Hash256, got: Hash256 },
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
pub const BEACON_ROLE_ENVELOPE_PROPOSER: BeaconRole = BeaconRole(9);
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
    /// Using bytes because the attestation wire shape varies by fork; `version` selects the
    /// decode shape (see [`DataVersion::decode_attestation`])
    pub aggregated_attestations:
        VariableList<VariableList<u8, MaxAggregatedAttestationBytes>, MaxCommitteeIndexes>,
    /// Validators selected as sync committee contributors with their selection proofs
    pub contributors: VariableList<AssignedAggregator, MaxContributors>,
    /// Sync committee contributions, one per subcommittee (4 total)
    pub sync_committee_contributions:
        VariableList<SyncCommitteeContribution<E>, MaxSyncContributions>,
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
    #[error("Failed to decode attestation: {0}")]
    AttestationDecodeError(ForkDecodeError),
    #[error("Data version mismatch: expected {expected:?}, got {got:?}")]
    VersionMismatch { expected: ForkName, got: ForkName },
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
        our_value: &AggregatorCommitteeConsensusData<E>,
    ) -> bool {
        // SIP-94 §2: `version` is leader-supplied and selects the decode shape (and thus the
        // signing-root merkleization), so bind it to our own candidate before any decoding.
        // The check lives here, not in `do_validation`, because `do_validation` is the
        // ssv-spec parity surface exercised directly by spec_tests; this binding is
        // Anchor-specific and must stay out of that entry point.
        let result = if value.version == our_value.version {
            self.do_validation(value)
        } else {
            Err(AggregatorCommitteeValidationError::VersionMismatch {
                expected: ForkName::from(our_value.version),
                got: ForkName::from(value.version),
            })
        };
        match result {
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
            value
                .version
                .decode_attestation::<E>(att_bytes)
                .map_err(AggregatorCommitteeValidationError::AttestationDecodeError)?;
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
            ForkName::Heze => 9,
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
            9 => ForkName::Heze,
            _ => return Err(DecodeError::NoMatchingVariant),
        }))
    }
}

/// Error from fork-aware decoding of consensus-data payload bytes.
#[derive(Error, Debug)]
pub enum ForkDecodeError {
    #[error("unable to decode ssz: {0:?}")]
    Decode(DecodeError),
    #[error("no wire shape pinned for fork {0}")]
    UnsupportedFork(ForkName),
}

/// The wire shape a fork selects for attestation-family containers.
enum WireShape {
    Base,
    Electra,
    Gloas,
}

impl DataVersion {
    /// Map this version to its attestation-family wire shape.
    ///
    /// The single home of the fork-to-shape equivalence classes; both decode helpers below go
    /// through it. The match is deliberately exhaustive: Heze has no wire shape pinned at the
    /// current Lighthouse pin and fails closed until one is.
    fn wire_shape(&self) -> Result<WireShape, ForkDecodeError> {
        match self.0 {
            ForkName::Base
            | ForkName::Altair
            | ForkName::Bellatrix
            | ForkName::Capella
            | ForkName::Deneb => Ok(WireShape::Base),
            ForkName::Electra | ForkName::Fulu => Ok(WireShape::Electra),
            ForkName::Gloas => Ok(WireShape::Gloas),
            ForkName::Heze => Err(ForkDecodeError::UnsupportedFork(ForkName::Heze)),
        }
    }

    /// The version to stamp on consensus data carrying this attestation, i.e. the inverse of
    /// [`Self::decode_attestation`]'s shape selection (each shape's representative fork).
    pub fn for_attestation_shape<E: EthSpec>(attestation: &Attestation<E>) -> Self {
        match attestation {
            Attestation::Base(_) => ForkName::Base.into(),
            Attestation::Electra(_) => ForkName::Electra.into(),
            Attestation::Gloas(_) => ForkName::Gloas.into(),
        }
    }

    /// Decode an SSZ `Attestation` with the wire shape this version selects.
    ///
    /// EIP-7688 makes the Gloas shapes serialization-compatible with Electra's, so decoding
    /// with the wrong shape can still succeed byte-for-byte; what changes is merkleization,
    /// so the wrong shape yields wrong signing roots on identical bytes (SIP-94 §2).
    pub fn decode_attestation<E: EthSpec>(
        &self,
        bytes: &[u8],
    ) -> Result<Attestation<E>, ForkDecodeError> {
        match self.wire_shape()? {
            WireShape::Base => AttestationBase::from_ssz_bytes(bytes)
                .map(Attestation::Base)
                .map_err(ForkDecodeError::Decode),
            WireShape::Electra => AttestationElectra::from_ssz_bytes(bytes)
                .map(Attestation::Electra)
                .map_err(ForkDecodeError::Decode),
            WireShape::Gloas => AttestationGloas::from_ssz_bytes(bytes)
                .map(Attestation::Gloas)
                .map_err(ForkDecodeError::Decode),
        }
    }

    /// Decode an SSZ `AggregateAndProof` with the wire shape this version selects.
    ///
    /// See [`Self::decode_attestation`] for the shape-selection rules.
    pub fn decode_aggregate_and_proof<E: EthSpec>(
        &self,
        bytes: &[u8],
    ) -> Result<AggregateAndProof<E>, ForkDecodeError> {
        match self.wire_shape()? {
            WireShape::Base => AggregateAndProofBase::from_ssz_bytes(bytes)
                .map(AggregateAndProof::Base)
                .map_err(ForkDecodeError::Decode),
            WireShape::Electra => AggregateAndProofElectra::from_ssz_bytes(bytes)
                .map(AggregateAndProof::Electra)
                .map_err(ForkDecodeError::Decode),
            WireShape::Gloas => AggregateAndProofGloas::from_ssz_bytes(bytes)
                .map(AggregateAndProof::Gloas)
                .map_err(ForkDecodeError::Decode),
        }
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
            ForkName::Heze => 9,
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
            ForkName::Heze => 9,
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

/// QBFT consensus value for committee attestation duties at Gloas-and-later forks.
///
/// Mirrors `BeaconVote` plus `attestation_data_index`, the BN-supplied
/// `AttestationData.index` field. Under Gloas this field encodes the attester's
/// fork-choice view of payload status (`0` = `EMPTY`, `1` = `FULL` for non-same-slot
/// attestations), is part of the signed attestation root, and therefore must
/// travel through QBFT rather than being reconstructed locally.
///
/// `GloasBeaconVote` and `BeaconVote` are kept as separate types so their SSZ
/// wire bytes mutually reject on length mismatch across the fork boundary.
#[derive(Clone, Debug, TreeHash, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct GloasBeaconVote {
    /// LMD-GHOST vote: root of the beacon block being attested to.
    pub block_root: Hash256,
    /// FFG source checkpoint, copied from the BN-supplied `AttestationData`.
    pub source: Checkpoint,
    /// FFG target checkpoint, copied from the BN-supplied `AttestationData`.
    pub target: Checkpoint,
    /// BN-supplied `AttestationData.index`. Under Gloas this encodes the
    /// attester's fork-choice view of payload status (`0` = `EMPTY`,
    /// `1` = `FULL` for non-same-slot attestations) and participates in the
    /// signed attestation root, so it must travel through QBFT rather than
    /// being reconstructed locally.
    pub attestation_data_index: u64,
}

impl QbftData for GloasBeaconVote {
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

/// Gloas variant of [`BeaconVoteValidator`]. Carries over every pre-Gloas check and
/// adds the two Gloas rules from [SIP-94][sip-94]: `attestation_data_index` is
/// range-checked to `{0, 1}`, and the slashing-DB check reconstructs `AttestationData`
/// with the decided index so cross-`index` double-votes trip protection. The index is
/// trusted from the QBFT leader, never compared against the local BN view. Rationale
/// for each rule is inline at its check.
///
/// [sip-94]: https://github.com/ssvlabs/SIPs/blob/7e8b5bd6d4007682d8bd75b06a2f2ac7b617e9e5/sips/epbs_support.md#2-modified-attestation-duty
pub struct GloasBeaconVoteValidator<E: EthSpec> {
    slot: Slot,
    // `None` if slashing protection is disabled via CLI.
    slashing_database: Option<Arc<SlashingDatabase>>,
    spec: Arc<ChainSpec>,
    validator_attestation_committees: HashMap<PublicKeyBytes, u64>,
    genesis_validators_root: Hash256,
    strict_mfp: bool,
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> QbftDataValidator<GloasBeaconVote> for GloasBeaconVoteValidator<E> {
    fn validate(&self, value: &GloasBeaconVote, our_value: &GloasBeaconVote) -> bool {
        match self.do_validation(value, our_value) {
            Ok(_) => true,
            Err(err) => {
                warn!(%err, "Operator proposed invalid gloas beacon vote");
                false
            }
        }
    }
}

impl<E: EthSpec> GloasBeaconVoteValidator<E> {
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
        value: &GloasBeaconVote,
        our_value: &GloasBeaconVote,
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

        // Gloas range-check (SIP-94): `index` encodes payload status, restricted to
        // `0` (EMPTY) or `1` (FULL). The same-slot `index = 0` rule is BN/gossip-enforced,
        // not checked here (it would need a BN lookup).
        if value.attestation_data_index >= 2 {
            return Err(BeaconVoteValidationError::IndexOutOfRange(
                value.attestation_data_index,
            ));
        }

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
        value: &GloasBeaconVote,
        our_value: &GloasBeaconVote,
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
        value: &GloasBeaconVote,
        our_value: &GloasBeaconVote,
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

    /// Per-validator slashing-DB check. The reconstructed `AttestationData.index` is the
    /// single QBFT-decided `attestation_data_index`, not a per-validator committee index,
    /// so the same `AttestationData` (and signing root) is checked for every validator.
    fn check_attestation_slashing(
        &self,
        value: &GloasBeaconVote,
    ) -> Result<(), BeaconVoteValidationError> {
        let Some(slashing_database) = &self.slashing_database else {
            return Ok(());
        };

        let attestation_data = AttestationData {
            slot: self.slot,
            index: value.attestation_data_index,
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

        // Only the validator keys are read here; the committee-index values are intentionally
        // unused because the reconstruction above uses the single QBFT-decided index.
        for validator_pubkey in self.validator_attestation_committees.keys() {
            slashing_database
                .preliminary_check_attestation(validator_pubkey, &attestation_data, domain_hash)
                .map_err(BeaconVoteValidationError::SlashableAttestation)?;
        }

        Ok(())
    }
}

/// Validator for `EnvelopeConsensusData` during envelope-signing QBFT (SIP-94 §6, EIP-7732 ePBS).
///
/// Envelope signing is NOT slashable (no Domain check, no slashing DB interaction).
/// Checks: (1) duty metadata (slot, validator_index, pubkey); (2) `builder_index` is
/// `BUILDER_INDEX_SELF_BUILD` (u64::MAX, self-built envelope); (3) `beacon_block_root`
/// matches the decided block root from beacon block QBFT.
pub struct EnvelopeConsensusDataValidator<E: EthSpec> {
    validator_pubkey: PublicKeyBytes,
    validator_index: ValidatorIndex,
    slot: Slot,
    decided_block_root: Hash256,
    _phantom: PhantomData<E>,
}

impl<E: EthSpec> EnvelopeConsensusDataValidator<E> {
    /// Construct a new validator with the given duty metadata and decided block root.
    pub fn new(
        validator_pubkey: PublicKeyBytes,
        validator_index: ValidatorIndex,
        slot: Slot,
        decided_block_root: Hash256,
    ) -> Self {
        Self {
            validator_pubkey,
            validator_index,
            slot,
            decided_block_root,
            _phantom: PhantomData,
        }
    }

    fn do_validation(&self, value: &EnvelopeConsensusData) -> Result<(), EnvelopeValidationError> {
        let blinded = value
            .decode_blinded_envelope::<E>()
            .map_err(EnvelopeValidationError::DecodeError)?;

        if value.duty.slot != self.slot {
            return Err(EnvelopeValidationError::SlotMismatch {
                expected: self.slot,
                got: value.duty.slot,
            });
        }

        if value.duty.validator_index != self.validator_index {
            return Err(EnvelopeValidationError::IndexMismatch {
                expected: self.validator_index,
                got: value.duty.validator_index,
            });
        }

        if value.duty.pub_key != self.validator_pubkey {
            return Err(EnvelopeValidationError::PubKeyMismatch {
                expected: self.validator_pubkey,
                got: value.duty.pub_key,
            });
        }

        if blinded.builder_index != BUILDER_INDEX_SELF_BUILD {
            return Err(EnvelopeValidationError::NotSelfBuild(blinded.builder_index));
        }

        if blinded.beacon_block_root != self.decided_block_root {
            return Err(EnvelopeValidationError::DecidedRootMismatch {
                expected: self.decided_block_root,
                got: blinded.beacon_block_root,
            });
        }

        Ok(())
    }
}

impl<E: EthSpec> QbftDataValidator<EnvelopeConsensusData> for EnvelopeConsensusDataValidator<E> {
    fn validate(&self, value: &EnvelopeConsensusData, _our_value: &EnvelopeConsensusData) -> bool {
        match self.do_validation(value) {
            Ok(_) => true,
            Err(err) => {
                warn!(%err, "Operator proposed invalid envelope consensus data");
                false
            }
        }
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
    /// Gloas-only: `GloasBeaconVote.attestation_data_index` was outside `{0, 1}`.
    /// Pre-Gloas votes carry no index field and never produce this error.
    #[error("Attestation data index out of range: {0}")]
    IndexOutOfRange(u64),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use bls::{AggregateSignature, FixedBytesExtended};
    use eth2::types::FullBlockContents;
    use ssz::ProgressiveBitList;
    use ssz_types::{BitList, BitVector};
    use types::{
        BeaconBlockDeneb, BeaconBlockGloas, Checkpoint, EmptyBlock, Epoch, ExecutionPayloadGloas,
        MainnetEthSpec, SyncCommitteeContribution, test_utils::generate_deterministic_keypair,
    };

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
                index,
                ..decode_test_attestation_data()
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
    // GloasBeaconVoteValidator Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Mirrors `create_test_validator`, slashing disabled. Slot 100 → current epoch 3.
    fn create_gloas_test_validator(strict_mfp: bool) -> GloasBeaconVoteValidator<MainnetEthSpec> {
        let spec = Arc::new(ChainSpec::mainnet());
        let validator_attestation_committees = HashMap::new();
        let genesis_validators_root = Hash256::zero();
        let slot = Slot::new(100);

        GloasBeaconVoteValidator::new(
            slot,
            None,
            spec,
            validator_attestation_committees,
            genesis_validators_root,
            strict_mfp,
        )
    }

    // ---------------------------------------------------------------------------------
    // A. Gloas range-check (`attestation_data_index` restricted to {0, 1})
    // ---------------------------------------------------------------------------------

    // Valid, matching source/target are used throughout section A so the only field
    // under test is `attestation_data_index`.
    #[test]
    fn test_gloas_index_zero_accepted() {
        let validator = create_gloas_test_validator(false);

        let source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: 0,
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: 0,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(
            result.is_ok(),
            "index 0 must be accepted, got error: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn test_gloas_index_one_accepted() {
        let validator = create_gloas_test_validator(false);

        let source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: 1,
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: 1,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(
            result.is_ok(),
            "index 1 must be accepted, got error: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn test_gloas_index_two_rejected() {
        let validator = create_gloas_test_validator(false);

        let source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: 2,
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: 2,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(result.is_err());
        match result.unwrap_err() {
            BeaconVoteValidationError::IndexOutOfRange(index) => assert_eq!(index, 2),
            err => panic!("Expected IndexOutOfRange(2), got: {:?}", err),
        }
    }

    #[test]
    fn test_gloas_index_large_rejected() {
        let validator = create_gloas_test_validator(false);

        let source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: u64::MAX,
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: u64::MAX,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(result.is_err());
        match result.unwrap_err() {
            BeaconVoteValidationError::IndexOutOfRange(index) => assert_eq!(index, u64::MAX),
            err => panic!("Expected IndexOutOfRange(u64::MAX), got: {:?}", err),
        }
    }

    // ---------------------------------------------------------------------------------
    // B. Carry-over checks: behave identically to `BeaconVoteValidator`.
    //    Each vote uses an in-range `attestation_data_index` so the new check is inert.
    // ---------------------------------------------------------------------------------

    #[test]
    fn test_gloas_mismatched_source_different_epochs() {
        // Port of `test_mismatched_source_different_epochs`.
        let validator = create_gloas_test_validator(false);

        let our_source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
            attestation_data_index: 0,
        };

        // Proposed vote with a different source epoch.
        let proposed_source = Checkpoint {
            epoch: Epoch::new(1),
            root: Hash256::from_low_u64_be(1),
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: proposed_source,
            target: our_target,
            attestation_data_index: 0,
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
    fn test_gloas_valid_source_equals_target_at_epoch_zero() {
        // Port: genesis exception allows source.epoch == target.epoch == 0.
        let validator = create_gloas_test_validator(false);

        let our_source = Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::from_low_u64_be(1),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
            attestation_data_index: 0,
        };

        let proposed_target = Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::from_low_u64_be(2),
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: proposed_target,
            attestation_data_index: 0,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(result.is_ok());
    }

    #[test]
    fn test_gloas_mismatched_target_different_epochs() {
        // Port of `test_mismatched_target_different_epochs`.
        let validator = create_gloas_test_validator(false);

        let our_source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
            attestation_data_index: 0,
        };

        // Proposed vote with a different (but still allowed: max is epoch 4) target epoch.
        let proposed_target = Checkpoint {
            epoch: Epoch::new(4),
            root: Hash256::from_low_u64_be(2),
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: proposed_target,
            attestation_data_index: 0,
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
    fn test_gloas_strict_mismatched_target_different_roots() {
        // Port: strict MFP still compares full checkpoints (roots included).
        let validator = create_gloas_test_validator(true);

        let our_source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
            attestation_data_index: 0,
        };

        // Same target epoch, different root.
        let proposed_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(999),
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: proposed_target,
            attestation_data_index: 0,
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
            err => panic!("Expected CheckpointMismatch error, got: {:?}", err),
        }
    }

    #[test]
    fn test_gloas_far_future_target_rejected() {
        // Current epoch is 3 (slot 100), so the max allowed target epoch is 4.
        // Target epoch 5 must be rejected before any later check runs.
        let validator = create_gloas_test_validator(false);

        let source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let target = Checkpoint {
            epoch: Epoch::new(5),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: 0,
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source,
            target,
            attestation_data_index: 0,
        };

        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(result.is_err());
        match result.unwrap_err() {
            BeaconVoteValidationError::FarFutureTargetEpoch(_) => {}
            err => panic!("Expected FarFutureTargetEpoch error, got: {:?}", err),
        }
    }

    #[test]
    fn test_gloas_mfp_ignores_index() {
        // SC-2: `attestation_data_index` must NOT be folded into majority-fork
        // protection. Identical source/target with differing in-range indices must
        // pass under both non-strict and strict MFP.
        for strict_mfp in [false, true] {
            let validator = create_gloas_test_validator(strict_mfp);

            let source = Checkpoint {
                epoch: Epoch::new(2),
                root: Hash256::from_low_u64_be(1),
            };
            let target = Checkpoint {
                epoch: Epoch::new(3),
                root: Hash256::from_low_u64_be(2),
            };
            let our_vote = GloasBeaconVote {
                block_root: Hash256::random(),
                source,
                target,
                attestation_data_index: 0,
            };
            // Identical source/target, different index.
            let proposed_vote = GloasBeaconVote {
                block_root: Hash256::random(),
                source,
                target,
                attestation_data_index: 1,
            };

            let result = validator.do_validation(&proposed_vote, &our_vote);
            assert!(
                result.is_ok(),
                "MFP (strict_mfp={strict_mfp}) must ignore attestation_data_index, got error: {:?}",
                result.unwrap_err()
            );
        }
    }

    /// Ports the non-strict root-mismatch sibling (`test_valid_matching_epochs_different_roots`)
    /// for Gloas. Non-strict `epoch_majority_fork_protection` keys on source/target EPOCHS
    /// only, so a proposed vote with matching epochs but different roots (reorg) must be
    /// accepted. `attestation_data_index` is held at 0 on both votes to isolate the root
    /// difference from the index.
    #[test]
    fn test_gloas_valid_matching_epochs_different_roots() {
        let validator = create_gloas_test_validator(false);

        let our_source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let our_target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };
        let our_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: our_source,
            target: our_target,
            attestation_data_index: 0,
        };

        // Proposed vote has same epochs but different roots (simulating reorg).
        let proposed_source = Checkpoint {
            epoch: Epoch::new(2),                // Same epoch
            root: Hash256::from_low_u64_be(999), // Different root
        };
        let proposed_target = Checkpoint {
            epoch: Epoch::new(3),                // Same epoch
            root: Hash256::from_low_u64_be(888), // Different root
        };
        let proposed_vote = GloasBeaconVote {
            block_root: Hash256::random(),
            source: proposed_source,
            target: proposed_target,
            attestation_data_index: 0,
        };

        // This should succeed since epochs match (roots don't need to match).
        let result = validator.do_validation(&proposed_vote, &our_vote);
        assert!(
            result.is_ok(),
            "Expected validation to succeed for matching epochs with different roots, got error: {:?}",
            result.unwrap_err()
        );
    }

    // ---------------------------------------------------------------------------------
    // C. Slashing-DB reconstruction: cross-`index` equivocation must trip protection.
    //    Lighthouse's `SlashingDatabase` is file-only, so this uses a real temp-file DB.
    // ---------------------------------------------------------------------------------

    #[test]
    fn test_gloas_slashing_trips_on_cross_index_equivocation() {
        use slashing_protection::SlashingDatabase;

        let tempdir = tempfile::tempdir().expect("create tempdir");
        let db_path = tempdir.path().join("slashing.sqlite");
        let db = SlashingDatabase::create(&db_path).expect("create slashing DB");

        let pubkey = generate_deterministic_keypair(0).pk.compress();
        db.register_validator(pubkey).expect("register validator");

        let slot = Slot::new(100);
        let block_root = Hash256::from_low_u64_be(0xbeef);
        let source = Checkpoint {
            epoch: Epoch::new(2),
            root: Hash256::from_low_u64_be(1),
        };
        let target = Checkpoint {
            epoch: Epoch::new(3),
            root: Hash256::from_low_u64_be(2),
        };

        // Domain must match the validator's own computation, else the trip would be
        // caused by a domain mismatch instead of the index.
        let spec = Arc::new(ChainSpec::mainnet());
        let genesis_validators_root = Hash256::zero();
        let epoch = slot.epoch(MainnetEthSpec::slots_per_epoch());
        let domain = spec.get_domain(
            epoch,
            Domain::BeaconAttester,
            &spec.fork_at_epoch(epoch),
            genesis_validators_root,
        );

        // Seed a record at index 0.
        let seed_attestation = AttestationData {
            slot,
            index: 0,
            beacon_block_root: block_root,
            source,
            target,
        };
        db.with_transaction(|txn| {
            db.check_and_insert_attestation(&pubkey, &seed_attestation, domain, txn)
        })
        .expect("seed attestation");

        // Committee-index `7` (neither 0 nor 1): if the reconstruction wrongly used this
        // instead of the decided index, the index-0 control below would fail.
        let mut committees_map = HashMap::new();
        committees_map.insert(pubkey, 7u64);
        let validator = GloasBeaconVoteValidator::<MainnetEthSpec>::new(
            slot,
            Some(Arc::new(db)),
            spec,
            committees_map,
            genesis_validators_root,
            false,
        );

        let our_value = GloasBeaconVote {
            block_root,
            source,
            target,
            attestation_data_index: 0,
        };

        // Control: index 0 matches the seed -> `Safe::SameData` -> Ok.
        let control = GloasBeaconVote {
            block_root,
            source,
            target,
            attestation_data_index: 0,
        };
        let control_result = validator.do_validation(&control, &our_value);
        assert!(
            control_result.is_ok(),
            "index-0 control must be Ok (Safe::SameData), got error: {:?}",
            control_result.unwrap_err()
        );

        // Trip: index 1 -> different signing root, same target epoch -> double vote.
        let equivocation = GloasBeaconVote {
            block_root,
            source,
            target,
            attestation_data_index: 1,
        };
        let trip_result = validator.do_validation(&equivocation, &our_value);
        assert!(trip_result.is_err());
        match trip_result.unwrap_err() {
            BeaconVoteValidationError::SlashableAttestation(_) => {}
            err => panic!("Expected SlashableAttestation error, got: {:?}", err),
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
    // GloasBeaconVote Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    fn create_gloas_beacon_vote(
        block_root: Hash256,
        source_epoch: u64,
        source_root: Hash256,
        target_epoch: u64,
        target_root: Hash256,
        attestation_data_index: u64,
    ) -> GloasBeaconVote {
        GloasBeaconVote {
            block_root,
            source: Checkpoint {
                epoch: Epoch::new(source_epoch),
                root: source_root,
            },
            target: Checkpoint {
                epoch: Epoch::new(target_epoch),
                root: target_root,
            },
            attestation_data_index,
        }
    }

    #[test]
    fn test_gloas_beacon_vote_ssz_roundtrip() {
        // Arrange: cover the `attestation_data_index` range boundaries (SSZ must round-trip any
        // u64).
        let block_root = Hash256::from_low_u64_be(0xabcd);
        let source_root = Hash256::from_low_u64_be(0x1111);
        let target_root = Hash256::from_low_u64_be(0x2222);

        for attestation_data_index in [0u64, 1, u64::MAX] {
            let vote = create_gloas_beacon_vote(
                block_root,
                3,
                source_root,
                4,
                target_root,
                attestation_data_index,
            );

            // Act
            let encoded = vote.as_ssz_bytes();
            let decoded = GloasBeaconVote::from_ssz_bytes(&encoded)
                .expect("SSZ-encoded `GloasBeaconVote` must decode");

            // Assert
            assert_eq!(vote, decoded);
        }
    }

    #[test]
    fn test_gloas_beacon_vote_hash_deterministic() {
        // Arrange: two independently-constructed votes with the same field values.
        let block_root = Hash256::from_low_u64_be(0x1234);
        let source_root = Hash256::from_low_u64_be(0x5555);
        let target_root = Hash256::from_low_u64_be(0x7777);
        let vote = create_gloas_beacon_vote(block_root, 3, source_root, 4, target_root, 0);
        let same = create_gloas_beacon_vote(block_root, 3, source_root, 4, target_root, 0);

        // Act + Assert: identical field values must hash identically (catches
        // identity- or address-dependent hashing).
        assert_eq!(vote.hash(), same.hash());

        // hash() must literally be SHA-256 over the SSZ bytes — this is the
        // cross-operator agreement contract: every operator independently
        // SSZ-encodes their `GloasBeaconVote` and hashes the bytes.
        let expected = {
            let mut hasher = Sha256::new();
            hasher.update(vote.as_ssz_bytes());
            Hash256::from(<[u8; 32]>::from(hasher.finalize()))
        };
        assert_eq!(
            vote.hash(),
            expected,
            "hash() must be SHA-256 over SSZ bytes"
        );

        // Flip block_root.
        let flipped_block_root = create_gloas_beacon_vote(
            Hash256::from_low_u64_be(0x9999),
            3,
            source_root,
            4,
            target_root,
            0,
        );
        assert_ne!(vote.hash(), flipped_block_root.hash());

        // Flip source.epoch.
        let flipped_source_epoch =
            create_gloas_beacon_vote(block_root, 99, source_root, 4, target_root, 0);
        assert_ne!(vote.hash(), flipped_source_epoch.hash());

        // Flip target.epoch.
        let flipped_target_epoch =
            create_gloas_beacon_vote(block_root, 3, source_root, 99, target_root, 0);
        assert_ne!(vote.hash(), flipped_target_epoch.hash());

        // Flip attestation_data_index: index=0 vs index=1 MUST differ — this is the
        // cross-index equivocation protection #1025 unlocks.
        let index_one = create_gloas_beacon_vote(block_root, 3, source_root, 4, target_root, 1);
        assert_ne!(
            vote.hash(),
            index_one.hash(),
            "index=0 and index=1 must produce distinct hashes (cross-index equivocation guard)"
        );
    }

    /// The QBFT-decided hash is the partial-signature base and binds the SSV signing
    /// root; for cluster-wide root agreement it MUST be sensitive to
    /// `attestation_data_index`. This isolates that single-field binding: two votes that
    /// agree on `block_root`/`source`/`target` and differ ONLY in
    /// `attestation_data_index` must hash differently, while two votes equal in every
    /// field must hash identically. (#1027 C3 support / #1061 hash-equality. The
    /// index=0-vs-1 case is also exercised inside `test_gloas_beacon_vote_hash_deterministic`;
    /// this test pins the binding contract on its own.)
    #[test]
    fn test_gloas_beacon_vote_hash_binds_attestation_data_index() {
        // Arrange: a fixed (block_root, source, target) baseline shared by all votes so
        // `attestation_data_index` is the only variable across the index pair.
        let block_root = Hash256::from_low_u64_be(0xb10c);
        let source_root = Hash256::from_low_u64_be(0x5005);
        let target_root = Hash256::from_low_u64_be(0x7007);
        let source_epoch = 3u64;
        let target_epoch = 4u64;

        let index_zero = create_gloas_beacon_vote(
            block_root,
            source_epoch,
            source_root,
            target_epoch,
            target_root,
            0,
        );
        let index_one = create_gloas_beacon_vote(
            block_root,
            source_epoch,
            source_root,
            target_epoch,
            target_root,
            1,
        );
        let index_zero_again = create_gloas_beacon_vote(
            block_root,
            source_epoch,
            source_root,
            target_epoch,
            target_root,
            0,
        );

        // Act
        let hash_zero = index_zero.hash();
        let hash_one = index_one.hash();
        let hash_zero_again = index_zero_again.hash();

        // Assert: differing only in `attestation_data_index` must change the decided hash.
        assert_ne!(
            hash_zero, hash_one,
            "votes differing only in attestation_data_index (0 vs 1) must hash differently \
             so the decided/signing root binds the index"
        );

        // Assert: fully identical fields must produce identical hashes (no
        // identity/address dependence in the hash).
        assert_eq!(
            hash_zero, hash_zero_again,
            "votes with fully identical fields must hash identically"
        );
    }

    #[test]
    fn test_beacon_vote_rejects_gloas_bytes() {
        // Arrange: encode a `GloasBeaconVote` (120 bytes).
        let gloas_vote = create_gloas_beacon_vote(
            Hash256::from_low_u64_be(0xdead),
            1,
            Hash256::from_low_u64_be(0xbeef),
            2,
            Hash256::from_low_u64_be(0xcafe),
            7,
        );
        let bytes = gloas_vote.as_ssz_bytes();
        assert_eq!(bytes.len(), 120);

        // Act: try to decode those bytes as the pre-Gloas `BeaconVote`.
        let result = BeaconVote::from_ssz_bytes(&bytes);

        // Assert: SIP §2 length-mismatch mutual rejection — pre-Gloas binaries
        // must not silently accept Gloas-shaped wire bytes.
        assert!(
            result.is_err(),
            "BeaconVote (112-byte fixed) must reject 120-byte GloasBeaconVote encoding"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // ProposerConsensusData Gloas (EIP-7732) Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Creates a minimal proposer `ValidatorDuty` at `slot` for block proposal tests.
    fn test_proposer_duty(slot: Slot) -> ValidatorDuty {
        ValidatorDuty {
            r#type: BEACON_ROLE_PROPOSER,
            pub_key: PublicKeyBytes::empty(),
            slot,
            validator_index: ValidatorIndex(0),
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        }
    }

    #[test]
    /// Tests that BeaconBlock::from_ssz_bytes_for_fork round-trips successfully through the
    /// SSZ bytes for a Gloas block variant.
    fn decode_block_round_trip() {
        let spec = ChainSpec::mainnet();
        let block = BeaconBlock::Gloas(BeaconBlockGloas::<MainnetEthSpec>::empty(&spec));

        let consensus_data =
            proposer_consensus_data(Slot::new(0), ForkName::Gloas, block.as_ssz_bytes());

        let decoded = consensus_data
            .decode_block::<MainnetEthSpec>()
            .expect("Gloas block should decode from DataSSZ");

        assert_eq!(
            decoded, block,
            "decoded Gloas block should equal the original block"
        );
    }

    #[test]
    /// Tests that `decode_blinded_block` rejects Gloas input with `DecodeError::NoMatchingVariant`.
    fn decode_blinded_block_rejects_gloas() {
        let spec = ChainSpec::mainnet();
        let consensus_data = proposer_consensus_data(
            Slot::new(0),
            ForkName::Gloas,
            gloas_block_bytes(&spec, Slot::new(0)),
        );

        let result = consensus_data.decode_blinded_block::<MainnetEthSpec>();

        assert!(
            matches!(result, Err(DecodeError::NoMatchingVariant)),
            "decode_blinded_block must reject Gloas with DecodeError::NoMatchingVariant, got {result:?}"
        );
    }

    #[test]
    /// Tests `DataVersion` and `ForkName` are deducible from Gloas encoded bytes.
    fn data_version_gloas_ssz_round_trip() {
        let version = DataVersion::from(ForkName::Gloas);

        let encoded = version.as_ssz_bytes();
        let decoded =
            DataVersion::from_ssz_bytes(&encoded).expect("Gloas DataVersion should decode");

        assert_eq!(
            decoded, version,
            "DataVersion should round-trip through SSZ for Gloas"
        );
        assert_eq!(
            ForkName::from(decoded),
            ForkName::Gloas,
            "decoded DataVersion should map back to ForkName::Gloas"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // validate_block_proposal Fork-Branch Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Builds a `ProposerConsensusDataValidator` over `spec` with the given slashing
    /// protection setting.
    ///
    /// The `SlashingDatabase` is constructed (no path-less constructor exists) but stays empty.
    /// The caller must keep the returned `TempDir` alive for the lifetime of the validator.
    fn test_block_proposal_validator(
        spec: Arc<ChainSpec>,
        disable_slashing_protection: bool,
    ) -> (
        tempfile::TempDir,
        ProposerConsensusDataValidator<MainnetEthSpec>,
    ) {
        let dir = tempfile::TempDir::new().expect("tempdir should succeed");
        let slashing_db = Arc::new(
            SlashingDatabase::open_or_create(&dir.path().join("slashing.sqlite"))
                .expect("slashing DB should open"),
        );
        let validator = ProposerConsensusDataValidator::<MainnetEthSpec>::new(
            slashing_db,
            disable_slashing_protection,
            spec,
            PublicKeyBytes::empty(),
            Hash256::zero(),
        );
        (dir, validator)
    }

    /// Gloas activation epoch used by [`gloas_scheduled_spec`]: past every fork mainnet already
    /// schedules (Deneb 269568, Electra 364032, Fulu 411392), so each era is reachable by slot
    /// choice.
    const GLOAS_TEST_EPOCH: u64 = 500000;

    /// Mainnet-based spec with Gloas scheduled at [`GLOAS_TEST_EPOCH`].
    fn gloas_scheduled_spec() -> ChainSpec {
        let mut spec = ChainSpec::mainnet();
        spec.gloas_fork_epoch = Some(Epoch::new(GLOAS_TEST_EPOCH));
        spec
    }

    /// A slot in the Gloas era of [`gloas_scheduled_spec`].
    fn gloas_era_slot() -> Slot {
        Epoch::new(GLOAS_TEST_EPOCH).start_slot(MainnetEthSpec::slots_per_epoch())
    }

    /// A slot in mainnet's Deneb era (epoch 300000: >= Deneb's 269568, < Electra's 364032).
    fn deneb_era_slot() -> Slot {
        Epoch::new(300000).start_slot(MainnetEthSpec::slots_per_epoch())
    }

    /// Builds a proposer `ProposerConsensusData` at `slot`, stamped with `fork` and carrying
    /// `data_ssz`.
    fn proposer_consensus_data(
        slot: Slot,
        fork: ForkName,
        data_ssz: Vec<u8>,
    ) -> ProposerConsensusData {
        ProposerConsensusData {
            duty: test_proposer_duty(slot),
            version: DataVersion::from(fork),
            data_ssz: VariableList::new(data_ssz).expect("block bytes should fit in DataSSZ"),
        }
    }

    /// SSZ bytes of an empty Gloas `BeaconBlock` whose internal slot equals `slot`.
    fn gloas_block_bytes(spec: &ChainSpec, slot: Slot) -> Vec<u8> {
        let mut block = BeaconBlockGloas::<MainnetEthSpec>::empty(spec);
        block.slot = slot;
        BeaconBlock::Gloas(block).as_ssz_bytes()
    }

    /// SSZ bytes of an empty Deneb `FullBlockContents` whose block's internal slot equals `slot`.
    fn deneb_block_contents_bytes(spec: &ChainSpec, slot: Slot) -> Vec<u8> {
        let mut block = BeaconBlockDeneb::<MainnetEthSpec>::empty(spec);
        block.slot = slot;
        FullBlockContents::<MainnetEthSpec>::new(
            BeaconBlock::Deneb(block),
            Some((VariableList::empty(), VariableList::empty())),
        )
        .as_ssz_bytes()
    }

    #[test]
    /// Tests that decoding failures when processing garbage SSZ block bytes are correctly
    /// propagated by the Gloas branch. (Happy-path decode coverage lives in the
    /// `do_validation_accepts_matching_*` tests below, which reach the same branches.)
    fn validate_block_proposal_gloas_rejects_invalid_bytes() {
        let consensus_data =
            proposer_consensus_data(gloas_era_slot(), ForkName::Gloas, vec![0xff; 32]);

        let (_dir, validator) =
            test_block_proposal_validator(Arc::new(gloas_scheduled_spec()), true);

        assert!(
            validator.validate_block_proposal(&consensus_data).is_err(),
            "Gloas branch should reject undecodable block bytes"
        );
    }

    /// Asserts `result` is exactly `VersionMismatch { expected, got }`.
    fn assert_version_mismatch(
        result: Result<(), DataValidationError>,
        expected_fork: ForkName,
        got_fork: ForkName,
    ) {
        match result {
            Err(DataValidationError::VersionMismatch { expected, got }) => {
                assert_eq!(expected, expected_fork, "expected fork should match");
                assert_eq!(got, got_fork, "got fork should match");
            }
            other => panic!("expected VersionMismatch, got {other:?}"),
        }
    }

    /// Asserts `result` is exactly `BlockSlotMismatch { expected, got }`.
    fn assert_block_slot_mismatch(
        result: Result<(), DataValidationError>,
        expected_slot: Slot,
        got_slot: Slot,
    ) {
        match result {
            Err(DataValidationError::BlockSlotMismatch { expected, got }) => {
                assert_eq!(
                    expected, expected_slot,
                    "expected slot should be the duty slot"
                );
                assert_eq!(
                    got, got_slot,
                    "got slot should be the block's internal slot"
                );
            }
            other => panic!("expected BlockSlotMismatch, got {other:?}"),
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // do_validation SIP-94 §4 Version / Fork-Schedule Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    /// Tests that a value stamped Deneb at a Gloas-era duty slot is rejected with
    /// `VersionMismatch` (SIP-94 §4), not misdiagnosed as a decode failure. The
    /// protection-enabled leg proves the check fires before the slashing gate: the empty
    /// slashing DB would reject any proposal that reached it.
    fn do_validation_rejects_deneb_version_at_gloas_slot() {
        for disable_slashing_protection in [true, false] {
            let spec = gloas_scheduled_spec();
            let slot = gloas_era_slot();
            let our_value =
                proposer_consensus_data(slot, ForkName::Gloas, gloas_block_bytes(&spec, slot));
            // Well-formed Deneb bytes: rejection must come from the version check, not decoding.
            let value = proposer_consensus_data(
                slot,
                ForkName::Deneb,
                deneb_block_contents_bytes(&spec, slot),
            );

            let (_dir, validator) =
                test_block_proposal_validator(Arc::new(spec), disable_slashing_protection);
            let result = validator.do_validation(&value, &our_value);

            assert_version_mismatch(result, ForkName::Gloas, ForkName::Deneb);
        }
    }

    #[test]
    /// Tests that a value stamped Gloas at a Deneb-era duty slot is rejected with
    /// `VersionMismatch` (SIP-94 §4).
    fn do_validation_rejects_gloas_version_at_deneb_slot() {
        let spec = gloas_scheduled_spec();
        let slot = deneb_era_slot();
        let our_value = proposer_consensus_data(
            slot,
            ForkName::Deneb,
            deneb_block_contents_bytes(&spec, slot),
        );
        let value = proposer_consensus_data(slot, ForkName::Gloas, gloas_block_bytes(&spec, slot));

        let (_dir, validator) = test_block_proposal_validator(Arc::new(spec), true);
        let result = validator.do_validation(&value, &our_value);

        assert_version_mismatch(result, ForkName::Deneb, ForkName::Gloas);
    }

    #[test]
    /// Tests that a value stamped Gloas at a Gloas-era duty slot passes `do_validation`
    /// with well-formed Gloas block bytes.
    fn do_validation_accepts_matching_gloas_version_at_gloas_slot() {
        let spec = gloas_scheduled_spec();
        let slot = gloas_era_slot();
        let our_value =
            proposer_consensus_data(slot, ForkName::Gloas, gloas_block_bytes(&spec, slot));
        let value = our_value.clone();

        let (_dir, validator) = test_block_proposal_validator(Arc::new(spec), true);
        let result = validator.do_validation(&value, &our_value);

        assert!(
            result.is_ok(),
            "matching Gloas version at a Gloas-era slot should validate, got {:?}",
            result.err()
        );
    }

    #[test]
    /// Tests that a value stamped Deneb at a Deneb-era duty slot passes `do_validation`
    /// with well-formed `FullBlockContents` bytes (preserving the blinded-then-contents
    /// fallback).
    fn do_validation_accepts_matching_deneb_version_at_deneb_slot() {
        let spec = gloas_scheduled_spec();
        let slot = deneb_era_slot();
        let our_value = proposer_consensus_data(
            slot,
            ForkName::Deneb,
            deneb_block_contents_bytes(&spec, slot),
        );
        let value = our_value.clone();

        let (_dir, validator) = test_block_proposal_validator(Arc::new(spec), true);
        let result = validator.do_validation(&value, &our_value);

        assert!(
            result.is_ok(),
            "matching Deneb version at a Deneb-era slot should validate, got {:?}",
            result.err()
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // do_validation Block-Slot Pin Tests
    // ═══════════════════════════════════════════════════════════════════════════════

    #[test]
    /// Tests that a well-formed Gloas block whose internal slot differs from the duty slot is
    /// rejected with `BlockSlotMismatch`: the block is signed under its own header slot, so a
    /// leader embedding a block built for a different slot must not reach signing. The
    /// protection-enabled leg proves the pin fires before the slashing gate: the empty
    /// slashing DB would reject any proposal that reached it.
    fn do_validation_rejects_gloas_block_slot_mismatch() {
        for disable_slashing_protection in [true, false] {
            let spec = gloas_scheduled_spec();
            let slot = gloas_era_slot();
            let our_value =
                proposer_consensus_data(slot, ForkName::Gloas, gloas_block_bytes(&spec, slot));
            // Version matches the duty-slot fork; only the block's internal slot is off by one.
            let value =
                proposer_consensus_data(slot, ForkName::Gloas, gloas_block_bytes(&spec, slot + 1));

            let (_dir, validator) =
                test_block_proposal_validator(Arc::new(spec), disable_slashing_protection);
            let result = validator.do_validation(&value, &our_value);

            assert_block_slot_mismatch(result, slot, slot + 1);
        }
    }

    #[test]
    /// Tests that the block-slot pin also covers the pre-Gloas fallback arm: a well-formed
    /// Deneb `FullBlockContents` whose block's internal slot differs from the duty slot is
    /// rejected with `BlockSlotMismatch`.
    fn do_validation_rejects_deneb_block_slot_mismatch() {
        let spec = gloas_scheduled_spec();
        let slot = deneb_era_slot();
        let our_value = proposer_consensus_data(
            slot,
            ForkName::Deneb,
            deneb_block_contents_bytes(&spec, slot),
        );
        let value = proposer_consensus_data(
            slot,
            ForkName::Deneb,
            deneb_block_contents_bytes(&spec, slot + 1),
        );

        let (_dir, validator) = test_block_proposal_validator(Arc::new(spec), true);
        let result = validator.do_validation(&value, &our_value);

        assert_block_slot_mismatch(result, slot, slot + 1);
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // EnvelopeConsensusData tests (ePBS envelope-signing QBFT value)
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Slot used across the envelope fixtures; arbitrary but non-zero to catch a validator
    /// that ignores the configured slot.
    const ENVELOPE_TEST_SLOT: u64 = 42;
    /// Validator index used across the envelope fixtures.
    const ENVELOPE_TEST_VALIDATOR_INDEX: usize = 7;
    /// Alternative validator index used across the envelope fixtures.
    const ENVELOPE_OTHER_TEST_VALIDATOR_INDEX: usize = 8;

    /// Deterministic validator pubkey for the "matching" side of the value check.
    fn envelope_test_pubkey(validator_index: usize) -> PublicKeyBytes {
        generate_deterministic_keypair(validator_index)
            .pk
            .compress()
    }

    /// The beacon block root the validator treats as decided; a self-build envelope must
    /// carry this value in its `beacon_block_root`.
    fn envelope_test_decided_root() -> Hash256 {
        Hash256::from_low_u64_be(0xdec1_ded0)
    }

    /// Builds a small full `ExecutionPayloadEnvelope` with the given `builder_index` and
    /// `beacon_block_root`. Payload and requests stay at their (small) defaults so the blinded
    /// form is cheap to encode.
    fn envelope_test_full_envelope(
        builder_index: u64,
        beacon_block_root: Hash256,
    ) -> ExecutionPayloadEnvelope<MainnetEthSpec> {
        ExecutionPayloadEnvelope {
            payload: ExecutionPayloadGloas::<MainnetEthSpec>::default(),
            execution_requests: ExecutionRequestsGloas::<MainnetEthSpec>::default(),
            builder_index,
            beacon_block_root,
            parent_beacon_block_root: Hash256::from_low_u64_be(0x2222),
        }
    }

    /// Builds a `ValidatorDuty` with the envelope-relevant fields populated; the remaining
    /// committee fields are irrelevant to `EnvelopeConsensusDataValidator` and are zeroed.
    fn envelope_test_duty(
        role: BeaconRole,
        pub_key: PublicKeyBytes,
        slot: Slot,
        validator_index: ValidatorIndex,
    ) -> ValidatorDuty {
        ValidatorDuty {
            r#type: role,
            pub_key,
            slot,
            validator_index,
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        }
    }

    /// Wraps a blinded envelope into an `EnvelopeConsensusData` with the given duty, tagging
    /// it as Gloas.
    fn envelope_consensus_data(
        duty: ValidatorDuty,
        blinded: &BlindedExecutionPayloadEnvelope<MainnetEthSpec>,
    ) -> EnvelopeConsensusData {
        EnvelopeConsensusData {
            duty,
            version: DataVersion::from(ForkName::Gloas),
            data_ssz: VariableList::new(blinded.as_ssz_bytes())
                .expect("blinded envelope bytes should fit in DataSSZ"),
        }
    }

    /// Encodes a blinded envelope with the given `builder_index`/`beacon_block_root` into a
    /// `data_ssz` payload, for tests that mutate only the embedded envelope.
    fn envelope_data_ssz(
        builder_index: u64,
        beacon_block_root: Hash256,
    ) -> VariableList<u8, ProposerConsensusDataLen> {
        let full = envelope_test_full_envelope(builder_index, beacon_block_root);
        let blinded = BlindedExecutionPayloadEnvelope::from_full(&full);
        VariableList::new(blinded.as_ssz_bytes())
            .expect("blinded envelope bytes should fit in DataSSZ")
    }

    /// Produces a matched `(validator, value)` pair that passes `do_validation`: the duty's
    /// slot/index/pubkey equal the validator's, and the embedded blinded envelope is
    /// self-build with `beacon_block_root == decided_block_root`.
    fn valid_envelope_setup() -> (
        EnvelopeConsensusDataValidator<MainnetEthSpec>,
        EnvelopeConsensusData,
    ) {
        let pubkey = envelope_test_pubkey(ENVELOPE_TEST_VALIDATOR_INDEX);
        let slot = Slot::new(ENVELOPE_TEST_SLOT);
        let validator_index = ValidatorIndex(ENVELOPE_TEST_VALIDATOR_INDEX);
        let decided_root = envelope_test_decided_root();

        let validator = EnvelopeConsensusDataValidator::<MainnetEthSpec>::new(
            pubkey,
            validator_index,
            slot,
            decided_root,
        );

        let full = envelope_test_full_envelope(BUILDER_INDEX_SELF_BUILD, decided_root);
        let blinded = BlindedExecutionPayloadEnvelope::from_full(&full);
        let duty = envelope_test_duty(BEACON_ROLE_ENVELOPE_PROPOSER, pubkey, slot, validator_index);
        let value = envelope_consensus_data(duty, &blinded);

        (validator, value)
    }

    /// Build a full envelope, blind it, and assert:
    /// 1. `blinded.tree_hash_root() == full.tree_hash_root()`.
    /// 2. `payload_root` really is the payload's hash.
    /// 3. The blinded form survives SSZ encode/decode.
    #[test]
    fn blinded_execution_payload_envelope_root_parity() {
        // Construct a full ExecutionPayloadEnvelope with test data.
        let full_envelope = envelope_test_full_envelope(42, Hash256::from_low_u64_be(0x1111));

        // Create blinded envelope from full.
        let blinded = BlindedExecutionPayloadEnvelope::from_full(&full_envelope);

        // This equality is load-bearing for the envelope signing duty.
        assert_eq!(
            blinded.tree_hash_root(),
            full_envelope.tree_hash_root(),
            "BlindedExecutionPayloadEnvelope root must equal full ExecutionPayloadEnvelope root"
        );

        assert_eq!(
            blinded.payload_root,
            full_envelope.payload.tree_hash_root(),
            "payload_root must equal the full payload's tree-hash root"
        );

        let encoded = blinded.as_ssz_bytes();
        let decoded = BlindedExecutionPayloadEnvelope::<MainnetEthSpec>::from_ssz_bytes(&encoded)
            .expect("SSZ decode should succeed");
        assert_eq!(
            blinded, decoded,
            "SSZ round-trip must preserve BlindedExecutionPayloadEnvelope"
        );
        assert_eq!(
            blinded.tree_hash_root(),
            decoded.tree_hash_root(),
            "SSZ round-trip must preserve tree-hash root"
        );
    }

    /// Encode an EnvelopeConsensusData and decode it back, confirming the value, its root, and
    /// the nested blinded envelope inside data_ssz all come back identical.
    #[test]
    fn envelope_consensus_data_ssz_round_trip() {
        // EnvelopeConsensusData whose data_ssz holds a real blinded envelope.
        let full =
            envelope_test_full_envelope(BUILDER_INDEX_SELF_BUILD, envelope_test_decided_root());

        // Blind the full envelope.
        let blinded = BlindedExecutionPayloadEnvelope::from_full(&full);

        // Create a duty and a consensus data value.
        let duty = envelope_test_duty(
            BEACON_ROLE_ENVELOPE_PROPOSER,
            envelope_test_pubkey(ENVELOPE_TEST_VALIDATOR_INDEX),
            Slot::new(ENVELOPE_TEST_SLOT),
            ValidatorIndex(ENVELOPE_TEST_VALIDATOR_INDEX),
        );
        let consensus_data = envelope_consensus_data(duty, &blinded);

        // Encode and decode the consensus data.
        let encoded = consensus_data.as_ssz_bytes();
        let decoded = EnvelopeConsensusData::from_ssz_bytes(&encoded)
            .expect("SSZ-encoded EnvelopeConsensusData must decode");

        assert_eq!(
            consensus_data, decoded,
            "SSZ round-trip must preserve full EnvelopeConsensusData"
        );
        assert_eq!(
            consensus_data.tree_hash_root(),
            decoded.tree_hash_root(),
            "SSZ round-trip must preserve EnvelopeConsensusData tree-hash root"
        );

        let decoded_blinded = decoded
            .decode_blinded_envelope::<MainnetEthSpec>()
            .expect("round-tripped data_ssz must still decode as a blinded envelope");
        assert_eq!(
            blinded, decoded_blinded,
            "round-trip must preserve the embedded BlindedExecutionPayloadEnvelope"
        );
    }

    /// Tests that data_ssz's capacity is exactly 2^23 bytes (8 MiB).
    #[test]
    fn envelope_consensus_data_data_ssz_respects_8mib_bound() {
        /// `ProposerConsensusDataLen` is `2^23` bytes (8 MiB); `data_ssz` shares this bound.
        const ENVELOPE_DATA_SSZ_MAX_LEN: usize = 1 << 23;

        assert_eq!(
            VariableList::<u8, ProposerConsensusDataLen>::max_len(),
            ENVELOPE_DATA_SSZ_MAX_LEN,
            "data_ssz capacity must be ProposerConsensusDataLen = 2^23 bytes"
        );

        // Construct a max-length data_ssz.
        let at_max: VariableList<u8, ProposerConsensusDataLen> =
            VariableList::new(vec![0u8; ENVELOPE_DATA_SSZ_MAX_LEN])
                .expect("a data_ssz of exactly 2^23 bytes must be within bound");
        assert_eq!(
            at_max.len(),
            ENVELOPE_DATA_SSZ_MAX_LEN,
            "max-length data_ssz must be 2^23 length"
        );

        // Encode an EnvelopeConsensusData with a max-length data_ssz.
        let max_value = EnvelopeConsensusData {
            duty: envelope_test_duty(
                BEACON_ROLE_PROPOSER,
                envelope_test_pubkey(ENVELOPE_TEST_VALIDATOR_INDEX),
                Slot::new(ENVELOPE_TEST_SLOT),
                ValidatorIndex(ENVELOPE_TEST_VALIDATOR_INDEX),
            ),
            version: DataVersion::from(ForkName::Gloas),
            data_ssz: at_max,
        };
        assert!(
            max_value.as_ssz_bytes().len() >= ENVELOPE_DATA_SSZ_MAX_LEN,
            "an EnvelopeConsensusData with a max-length data_ssz must encode"
        );

        // Construct a data_ssz that is one byte greater than MAX.
        let over = VariableList::<u8, ProposerConsensusDataLen>::new(vec![
            0u8;
            ENVELOPE_DATA_SSZ_MAX_LEN
                + 1
        ]);
        assert!(
            matches!(over, Err(ssz_types::Error::OutOfBounds { .. })),
            "a data_ssz of 2^23 + 1 bytes must be rejected as out of bounds, got {over:?}"
        );
    }

    /// Tests that identical values hash identically and that permutating a single field changes the
    /// outcome. Determinism in building the consensus data.
    #[test]
    fn envelope_consensus_data_hash_is_deterministic() {
        // Arrange: two independently-built values with identical field values.
        let full =
            envelope_test_full_envelope(BUILDER_INDEX_SELF_BUILD, envelope_test_decided_root());
        let blinded = BlindedExecutionPayloadEnvelope::from_full(&full);
        let duty = envelope_test_duty(
            BEACON_ROLE_PROPOSER,
            envelope_test_pubkey(ENVELOPE_TEST_VALIDATOR_INDEX),
            Slot::new(ENVELOPE_TEST_SLOT),
            ValidatorIndex(ENVELOPE_TEST_VALIDATOR_INDEX),
        );
        let value = envelope_consensus_data(duty.clone(), &blinded);
        let same = envelope_consensus_data(duty.clone(), &blinded);

        assert_eq!(
            value.hash(),
            same.hash(),
            "identical EnvelopeConsensusData values must hash identically"
        );

        let mut mutated_duty = duty;
        mutated_duty.slot = Slot::new(ENVELOPE_TEST_SLOT + 1);
        let mutated = envelope_consensus_data(mutated_duty, &blinded);
        assert_ne!(
            value.hash(),
            mutated.hash(),
            "changing envelope consensus data field duty.slot must change the EnvelopeConsensusData hash"
        );
    }

    /// Tests that a well-formed self-build envelope passes both the internal check and the public
    /// validate().
    #[test]
    fn envelope_value_check_accepts_valid() {
        // Create a valid validator/value pair.
        let (validator, value) = valid_envelope_setup();

        // Valid pair passes do_validation and validate().
        let result = validator.do_validation(&value);
        assert!(
            result.is_ok(),
            "a well-formed self-build envelope value must pass, got {result:?}"
        );
        assert!(
            validator.validate(&value, &value),
            "validate() must return true for a well-formed envelope value"
        );
    }

    /// Tests that an otherwise valid envelope value with a duty slot differing from the validator's
    /// is rejected with SlotMismatch.
    #[test]
    fn envelope_value_check_rejects_wrong_slot() {
        // Create valid pair and mutate only the duty slot.
        let (validator, mut value) = valid_envelope_setup();
        value.duty.slot = Slot::new(ENVELOPE_TEST_SLOT + 1);

        // Validate.
        let result = validator.do_validation(&value);

        assert!(
            matches!(result, Err(EnvelopeValidationError::SlotMismatch { .. })),
            "a duty slot differing from the validator's must yield SlotMismatch, got {result:?}"
        );
    }

    /// Tests that an otherwise valid envelope value with a duty validator index differing from the
    /// validator's is rejected with IndexMismatch.
    #[test]
    fn envelope_value_check_rejects_wrong_validator_index() {
        // Create valid pair and mutate only the duty validator index.
        let (validator, mut value) = valid_envelope_setup();
        value.duty.validator_index = ValidatorIndex(ENVELOPE_TEST_VALIDATOR_INDEX + 1);

        // Validate.
        let result = validator.do_validation(&value);

        assert!(
            matches!(result, Err(EnvelopeValidationError::IndexMismatch { .. })),
            "a duty index differing from the validator's must yield IndexMismatch, got {result:?}"
        );
    }

    /// Tests that an otherwise valid envelope value with a duty pubkey differing from the
    /// validator's is rejected with PubKeyMismatch.
    #[test]
    fn envelope_value_check_rejects_wrong_pubkey() {
        // Create valid pair and mutate only the duty pubkey.
        let (validator, mut value) = valid_envelope_setup();
        value.duty.pub_key = envelope_test_pubkey(ENVELOPE_OTHER_TEST_VALIDATOR_INDEX);

        // Validate.
        let result = validator.do_validation(&value);

        assert!(
            matches!(result, Err(EnvelopeValidationError::PubKeyMismatch { .. })),
            "a duty pubkey differing from the validator's must yield PubKeyMismatch, got {result:?}"
        );
    }

    /// Tests that this validation path only exists for a self-build envelope.
    #[test]
    fn envelope_value_check_rejects_non_self_build() {
        // Create a valid envelope and rebuild with a non-self-build builder_index.
        let (validator, mut value) = valid_envelope_setup();
        value.data_ssz = envelope_data_ssz(0, envelope_test_decided_root());

        // Attempt to validate.
        let result = validator.do_validation(&value);

        assert!(
            matches!(result, Err(EnvelopeValidationError::NotSelfBuild(0))),
            "a non-self-build builder_index must yield NotSelfBuild, got {result:?}"
        );
        assert!(
            !validator.validate(&value, &value),
            "validate() must return false for a non-self-build envelope value"
        );
    }

    /// Test that a self-build envelope whose beacon_block_root doesn't match the decided root is
    /// rejected.
    #[test]
    fn envelope_value_check_rejects_wrong_decided_root() {
        // A self-build envelope bound to a different beacon block root than the decided value.
        let (validator, mut value) = valid_envelope_setup();
        let wrong_root = Hash256::from_low_u64_be(0xbad0);
        value.data_ssz = envelope_data_ssz(BUILDER_INDEX_SELF_BUILD, wrong_root);

        // Attempt to validate.
        let result = validator.do_validation(&value);

        assert!(
            matches!(
                result,
                Err(EnvelopeValidationError::DecidedRootMismatch { .. })
            ),
            "a beacon_block_root differing from the decided root must yield \
             DecidedRootMismatch, got {result:?}"
        );
    }

    /// Tests that a self-build envelope whose data_ssz is not decodable as a blinded envelope is
    /// rejected with DecodeError.
    #[test]
    fn envelope_value_check_rejects_undecodable_data_ssz() {
        // Create valid envelope and replace data_ssz with bytes that cannot decode as a blinded
        // envelope.
        let (validator, mut value) = valid_envelope_setup();
        value.data_ssz =
            VariableList::new(vec![0xFFu8; 3]).expect("3 garbage bytes fit within DataSSZ");

        // Attempt to validate.
        let result = validator.do_validation(&value);

        assert!(
            matches!(result, Err(EnvelopeValidationError::DecodeError(_))),
            "undecodable data_ssz must yield a DecodeError, got {result:?}"
        );

        // Additional check for error message contents.
        assert!(
            result
                .as_ref()
                .err()
                .unwrap()
                .to_string()
                .contains("EnvelopeConsensusData"),
            "decode error must be attributed to EnvelopeConsensusData, got: {result:?}"
        );
    }

    /// Tests that a value with a non-Gloas version and an unrelated duty.r#type (e.g. attester)
    /// pass validation.
    #[test]
    fn envelope_value_check_ignores_version_and_duty_type() {
        // Valid value whose version is not Gloas, duty role unrelated to envelope proposal.
        let (validator, mut value) = valid_envelope_setup();
        value.version = DataVersion::from(ForkName::Deneb);
        value.duty.r#type = BEACON_ROLE_ATTESTER;

        // Data version and duty role not part of the envelope value check.
        let result = validator.do_validation(&value);

        assert!(
            result.is_ok(),
            "envelope value check must ignore version and duty.r#type, got {result:?}"
        );
        assert!(
            validator.validate(&value, &value),
            "validate() must accept regardless of version and duty.r#type"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // DataVersion Fork-Aware Decode Helper Tests (EIP-7688 / SIP-94 §2)
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Forks whose attestation/aggregate wire shape is the Base one.
    const BASE_SHAPE_FORKS: [ForkName; 5] = [
        ForkName::Base,
        ForkName::Altair,
        ForkName::Bellatrix,
        ForkName::Capella,
        ForkName::Deneb,
    ];
    /// Forks whose attestation/aggregate wire shape is the Electra one.
    const ELECTRA_SHAPE_FORKS: [ForkName; 2] = [ForkName::Electra, ForkName::Fulu];

    /// Attestation data shared by the decode-helper fixtures.
    fn decode_test_attestation_data() -> AttestationData {
        AttestationData {
            slot: Slot::new(1000),
            index: 0,
            beacon_block_root: Hash256::zero(),
            source: Checkpoint {
                epoch: Epoch::new(10),
                root: Hash256::zero(),
            },
            target: Checkpoint {
                epoch: Epoch::new(11),
                root: Hash256::zero(),
            },
        }
    }

    /// Base-shaped attestation with one aggregation bit set.
    fn base_shape_attestation() -> AttestationBase<MainnetEthSpec> {
        let mut aggregation_bits = BitList::with_capacity(128).unwrap();
        aggregation_bits.set(0, true).unwrap();
        AttestationBase {
            aggregation_bits,
            data: decode_test_attestation_data(),
            signature: AggregateSignature::infinity(),
        }
    }

    /// Electra-shaped attestation with one aggregation bit and one committee bit set.
    fn electra_shape_attestation() -> AttestationElectra<MainnetEthSpec> {
        let mut aggregation_bits = BitList::with_capacity(128).unwrap();
        aggregation_bits.set(0, true).unwrap();
        let mut committee_bits = BitVector::default();
        committee_bits.set(5, true).unwrap();
        AttestationElectra {
            aggregation_bits,
            data: decode_test_attestation_data(),
            signature: AggregateSignature::infinity(),
            committee_bits,
        }
    }

    /// Gloas-shaped attestation with one aggregation bit and one committee bit set.
    fn gloas_shape_attestation() -> AttestationGloas<MainnetEthSpec> {
        let mut aggregation_bits = ProgressiveBitList::with_capacity(128);
        aggregation_bits.set(0, true).unwrap();
        let mut committee_bits = BitVector::default();
        committee_bits.set(5, true).unwrap();
        AttestationGloas {
            aggregation_bits,
            data: decode_test_attestation_data(),
            signature: AggregateSignature::infinity(),
            committee_bits,
        }
    }

    fn base_shape_aggregate_and_proof() -> AggregateAndProofBase<MainnetEthSpec> {
        AggregateAndProofBase {
            aggregator_index: 7,
            aggregate: base_shape_attestation(),
            selection_proof: Signature::empty(),
        }
    }

    fn electra_shape_aggregate_and_proof() -> AggregateAndProofElectra<MainnetEthSpec> {
        AggregateAndProofElectra {
            aggregator_index: 7,
            aggregate: electra_shape_attestation(),
            selection_proof: Signature::empty(),
        }
    }

    fn gloas_shape_aggregate_and_proof() -> AggregateAndProofGloas<MainnetEthSpec> {
        AggregateAndProofGloas {
            aggregator_index: 7,
            aggregate: gloas_shape_attestation(),
            selection_proof: Signature::empty(),
        }
    }

    #[test]
    fn decode_attestation_selects_base_shape_for_pre_electra_versions() {
        let bytes = base_shape_attestation().as_ssz_bytes();
        for fork in BASE_SHAPE_FORKS {
            let decoded = DataVersion::from(fork)
                .decode_attestation::<MainnetEthSpec>(&bytes)
                .unwrap_or_else(|e| panic!("{fork} must decode the Base shape: {e:?}"));
            assert!(
                matches!(decoded, Attestation::Base(_)),
                "{fork} must select the Base attestation shape"
            );
        }
    }

    #[test]
    fn decode_attestation_selects_electra_shape_for_electra_and_fulu() {
        let bytes = electra_shape_attestation().as_ssz_bytes();
        for fork in ELECTRA_SHAPE_FORKS {
            let decoded = DataVersion::from(fork)
                .decode_attestation::<MainnetEthSpec>(&bytes)
                .unwrap_or_else(|e| panic!("{fork} must decode the Electra shape: {e:?}"));
            assert!(
                matches!(decoded, Attestation::Electra(_)),
                "{fork} must select the Electra attestation shape"
            );
        }
    }

    #[test]
    fn decode_attestation_selects_gloas_shape_for_gloas() {
        let bytes = gloas_shape_attestation().as_ssz_bytes();
        let decoded = DataVersion::from(ForkName::Gloas)
            .decode_attestation::<MainnetEthSpec>(&bytes)
            .expect("Gloas must decode the Gloas shape");
        assert!(
            matches!(decoded, Attestation::Gloas(_)),
            "Gloas must select the Gloas attestation shape"
        );
    }

    #[test]
    fn decode_attestation_fails_closed_for_heze() {
        // Even well-formed bytes for the latest pinned shape must be rejected: Heze has no wire
        // shape pinned at the current Lighthouse pin.
        let bytes = gloas_shape_attestation().as_ssz_bytes();
        let result = DataVersion::from(ForkName::Heze).decode_attestation::<MainnetEthSpec>(&bytes);
        assert!(
            matches!(
                result,
                Err(ForkDecodeError::UnsupportedFork(ForkName::Heze))
            ),
            "Heze must fail closed, got {result:?}"
        );
    }

    #[test]
    fn decode_aggregate_and_proof_selects_base_shape_for_pre_electra_versions() {
        let bytes = base_shape_aggregate_and_proof().as_ssz_bytes();
        for fork in BASE_SHAPE_FORKS {
            let decoded = DataVersion::from(fork)
                .decode_aggregate_and_proof::<MainnetEthSpec>(&bytes)
                .unwrap_or_else(|e| panic!("{fork} must decode the Base shape: {e:?}"));
            assert!(
                matches!(decoded, AggregateAndProof::Base(_)),
                "{fork} must select the Base aggregate-and-proof shape"
            );
        }
    }

    #[test]
    fn decode_aggregate_and_proof_selects_electra_shape_for_electra_and_fulu() {
        let bytes = electra_shape_aggregate_and_proof().as_ssz_bytes();
        for fork in ELECTRA_SHAPE_FORKS {
            let decoded = DataVersion::from(fork)
                .decode_aggregate_and_proof::<MainnetEthSpec>(&bytes)
                .unwrap_or_else(|e| panic!("{fork} must decode the Electra shape: {e:?}"));
            assert!(
                matches!(decoded, AggregateAndProof::Electra(_)),
                "{fork} must select the Electra aggregate-and-proof shape"
            );
        }
    }

    #[test]
    fn decode_aggregate_and_proof_selects_gloas_shape_for_gloas() {
        let bytes = gloas_shape_aggregate_and_proof().as_ssz_bytes();
        let decoded = DataVersion::from(ForkName::Gloas)
            .decode_aggregate_and_proof::<MainnetEthSpec>(&bytes)
            .expect("Gloas must decode the Gloas shape");
        assert!(
            matches!(decoded, AggregateAndProof::Gloas(_)),
            "Gloas must select the Gloas aggregate-and-proof shape"
        );
    }

    #[test]
    fn decode_aggregate_and_proof_fails_closed_for_heze() {
        let bytes = gloas_shape_aggregate_and_proof().as_ssz_bytes();
        let result =
            DataVersion::from(ForkName::Heze).decode_aggregate_and_proof::<MainnetEthSpec>(&bytes);
        assert!(
            matches!(
                result,
                Err(ForkDecodeError::UnsupportedFork(ForkName::Heze))
            ),
            "Heze must fail closed, got {result:?}"
        );
    }

    /// EIP-7495 makes progressive containers serialization-compatible with their positional
    /// counterparts: the SAME Electra-shaped bytes decode successfully under BOTH the Electra
    /// and the Gloas shape. What changes is merkleization (positional vs progressive), so the
    /// two decoded values produce different tree hash roots on identical bytes. This is exactly
    /// why `version` must select the decode shape (SIP-94 §2): decode success alone cannot
    /// detect a shape mismatch.
    #[test]
    fn identical_attestation_bytes_decode_under_both_shapes_with_distinct_roots() {
        let bytes = electra_shape_attestation().as_ssz_bytes();

        let electra = DataVersion::from(ForkName::Electra)
            .decode_attestation::<MainnetEthSpec>(&bytes)
            .expect("Electra-shaped bytes must decode under the Electra shape");
        let gloas = DataVersion::from(ForkName::Gloas)
            .decode_attestation::<MainnetEthSpec>(&bytes)
            .expect("EIP-7495: the same bytes must also decode under the Gloas shape");

        assert!(matches!(electra, Attestation::Electra(_)));
        assert!(matches!(gloas, Attestation::Gloas(_)));
        // Serialization compatibility holds in both directions: both decodes re-encode to the
        // original bytes.
        assert_eq!(electra.as_ssz_bytes(), bytes);
        assert_eq!(gloas.as_ssz_bytes(), bytes);
        // Positional (Electra) vs progressive (Gloas) merkleization: the roots must differ.
        assert_ne!(
            electra.tree_hash_root(),
            gloas.tree_hash_root(),
            "identical bytes must merkleize differently under positional vs progressive shapes"
        );
    }

    /// Same as the attestation root test, for `AggregateAndProof`: identical Electra-shaped
    /// bytes decode under both shapes, but the signing root over the container differs, so an
    /// operator decoding with the wrong shape would sign a root its peers reject.
    #[test]
    fn identical_aggregate_bytes_decode_under_both_shapes_with_distinct_signing_roots() {
        let bytes = electra_shape_aggregate_and_proof().as_ssz_bytes();

        let electra = DataVersion::from(ForkName::Electra)
            .decode_aggregate_and_proof::<MainnetEthSpec>(&bytes)
            .expect("Electra-shaped bytes must decode under the Electra shape");
        let gloas = DataVersion::from(ForkName::Gloas)
            .decode_aggregate_and_proof::<MainnetEthSpec>(&bytes)
            .expect("EIP-7495: the same bytes must also decode under the Gloas shape");

        assert!(matches!(electra, AggregateAndProof::Electra(_)));
        assert!(matches!(gloas, AggregateAndProof::Gloas(_)));
        assert_eq!(electra.as_ssz_bytes(), bytes);
        assert_eq!(gloas.as_ssz_bytes(), bytes);
        assert_ne!(
            electra.tree_hash_root(),
            gloas.tree_hash_root(),
            "identical bytes must merkleize differently under positional vs progressive shapes"
        );
        let domain = Hash256::repeat_byte(0xD0);
        assert_ne!(
            electra.signing_root(domain),
            gloas.signing_root(domain),
            "the signing root over the container must differ between the two shapes"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // do_validation Aggregator-Branch Version Binding Tests (SIP-94 §2)
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Builds an aggregator-duty `ProposerConsensusData` stamped with `fork`, carrying
    /// `data_ssz`. The duty slot is arbitrary: the aggregator branch binds `version` to our own
    /// candidate, not to the fork schedule.
    fn aggregator_consensus_data(fork: ForkName, data_ssz: Vec<u8>) -> ProposerConsensusData {
        let mut duty = test_proposer_duty(Slot::new(1000));
        duty.r#type = BEACON_ROLE_AGGREGATOR;
        ProposerConsensusData {
            duty,
            version: DataVersion::from(fork),
            data_ssz: VariableList::new(data_ssz).expect("aggregate bytes should fit in DataSSZ"),
        }
    }

    #[test]
    /// Tests that the aggregator branch rejects a value whose leader-supplied `version` differs
    /// from our own candidate's, BEFORE any decoding: the value is well-formed for its claimed
    /// version, so the rejection can only come from the version binding.
    fn do_validation_rejects_aggregator_version_mismatch() {
        let our_value = aggregator_consensus_data(
            ForkName::Electra,
            electra_shape_aggregate_and_proof().as_ssz_bytes(),
        );
        let value = aggregator_consensus_data(
            ForkName::Deneb,
            base_shape_aggregate_and_proof().as_ssz_bytes(),
        );

        let (_dir, validator) = test_block_proposal_validator(Arc::new(ChainSpec::mainnet()), true);
        let result = validator.do_validation(&value, &our_value);

        assert_version_mismatch(result, ForkName::Electra, ForkName::Deneb);
    }

    #[test]
    /// Tests that the aggregator branch accepts a value whose `version` matches our candidate's
    /// and whose bytes decode under that version's shape.
    fn do_validation_accepts_aggregator_matching_version() {
        let our_value = aggregator_consensus_data(
            ForkName::Electra,
            electra_shape_aggregate_and_proof().as_ssz_bytes(),
        );
        let value = our_value.clone();

        let (_dir, validator) = test_block_proposal_validator(Arc::new(ChainSpec::mainnet()), true);
        let result = validator.do_validation(&value, &our_value);

        assert!(
            result.is_ok(),
            "matching aggregator version should validate, got {:?}",
            result.err()
        );
    }

    #[test]
    /// Tests that `AggregatorCommitteeDataValidator::validate` binds the leader-supplied
    /// `version` to our own candidate's (SIP-94 §2): a value that is valid on its own terms is
    /// still rejected when our candidate carries a different version.
    fn aggregator_committee_validator_rejects_version_mismatch() {
        let validator = create_aggregator_committee_validator();
        // Same fixture as our Deneb-stamped candidate below, differing only in the stamped
        // version and the matching (Electra-shaped) attestation bytes.
        let electra_value = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Electra),
            aggregated_attestations: VariableList::new(vec![
                VariableList::new(electra_shape_attestation().as_ssz_bytes()).unwrap(),
            ])
            .unwrap(),
            ..create_populated_consensus_data()
        };
        // Control: the value passes when our candidate carries the same version, so the
        // rejection below can only come from the version binding.
        assert!(
            validator.validate(&electra_value, &electra_value),
            "control: the value must be valid under a matching version"
        );

        // Our own candidate is stamped Deneb; the Electra-stamped value must be rejected.
        let our_value = create_populated_consensus_data();
        assert!(
            !validator.validate(&electra_value, &our_value),
            "validate() must return false when the value's version differs from our candidate's"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // Max-Size Gloas Aggregate vs MaxAggregatedAttestationBytes
    // ═══════════════════════════════════════════════════════════════════════════════

    /// `MaxAggregatedAttestationBytes` (131,308) comes from go-ssv's `ssz-max:"64,131308"`
    /// bound, derived pre-Gloas. A maximum-size Gloas aggregate (every committee bit set,
    /// aggregation bits spanning every validator in the slot) must still fit, otherwise Gloas
    /// aggregates could not be carried in `AggregatorCommitteeConsensusData`.
    #[test]
    fn max_size_gloas_aggregate_fits_aggregated_attestation_bound() {
        use typenum::Unsigned;

        let max_validators_per_slot = <MainnetEthSpec as EthSpec>::MaxValidatorsPerSlot::to_usize();
        let max_committees_per_slot = <MainnetEthSpec as EthSpec>::MaxCommitteesPerSlot::to_usize();

        let mut aggregation_bits = ProgressiveBitList::with_capacity(max_validators_per_slot);
        for i in 0..max_validators_per_slot {
            aggregation_bits.set(i, true).expect("bit within capacity");
        }
        let mut committee_bits = BitVector::default();
        for i in 0..max_committees_per_slot {
            committee_bits
                .set(i, true)
                .expect("committee bit within capacity");
        }

        let attestation = AttestationGloas::<MainnetEthSpec> {
            aggregation_bits,
            data: decode_test_attestation_data(),
            signature: AggregateSignature::infinity(),
            committee_bits,
        };

        let bytes = attestation.as_ssz_bytes();
        let bound = MaxAggregatedAttestationBytes::to_usize();
        assert!(
            bytes.len() <= bound,
            "max-size Gloas aggregate ({} bytes) must fit the {bound} byte bound",
            bytes.len()
        );
        assert!(
            VariableList::<u8, MaxAggregatedAttestationBytes>::new(bytes).is_ok(),
            "max-size Gloas aggregate must fit the aggregated_attestations element type"
        );
    }
}
