use bls::PublicKeyBytes;
use qbft_manager::QbftError;
use safe_arith::ArithError;
use signature_collector::CollectionError;
use ssv_types::{ClusterId, ValidatorIndex};
use ssz::DecodeError;
use types::Slot;
use validator_store::Error as ValidatorStoreError;

#[derive(Debug, Clone)]
pub enum SpecificError {
    Unsupported,
    SignatureCollectionFailed(CollectionError),
    ArithError(ArithError),
    QbftError(QbftError),
    Timeout,
    InvalidQbftData(DecodeError),
    TooManySyncSubnetsToSign,
    NoDataAgreed,
    Metadata,
    MissingIndex,
    SlotClock,
    NotSynced,
    InconsistentDatabase,
    /// Database inconsistency: validator references a cluster that doesn't exist
    ValidatorClusterMismatch {
        validator_pubkey: PublicKeyBytes,
        cluster_id: ClusterId,
    },
    KeyShareDecryptionFailed,
    DataTooLarge(String),
    ClusterLiquidated,
    /// Requested slot has already passed the current cached slot in `VotingAssignments`
    MetadataSlotPassed,
    /// Watch channel for `VotingAssignments` has been closed
    MetadataChannelClosed,
    /// Requested slot has already passed the current cached slot in `AggregationAssignments`
    AggregatorInfoSlotPassed,
    /// Watch channel for `AggregationAssignments` has been closed
    AggregatorInfoChannelClosed,
    /// `produce_selection_proof` called for validator not in
    /// `VotingAssignments.attesting_committees`
    ValidatorNotAttesting {
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    },
    /// `produce_sync_selection_proof` called for validator not in
    /// `VotingAssignments.sync_validators_by_subnet`
    ValidatorNotInSyncCommittee {
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    },
    /// Pre-built consensus data not found for this committee (Boole+)
    ConsensusDataNotFound,
    /// This committee's aggregate not found in consensus data (Boole+)
    AggregateNotInConsensus(u64),
    /// This subcommittee's contribution not found in consensus data (Boole+)
    ContributionNotInConsensus(u64),
    /// This validator not found in consensus data (Boole+)
    ValidatorNotInConsensus(ValidatorIndex),
}

impl From<CollectionError> for SpecificError {
    fn from(err: CollectionError) -> SpecificError {
        SpecificError::SignatureCollectionFailed(err)
    }
}

impl From<ArithError> for SpecificError {
    fn from(err: ArithError) -> SpecificError {
        SpecificError::ArithError(err)
    }
}

impl From<QbftError> for SpecificError {
    fn from(err: QbftError) -> SpecificError {
        SpecificError::QbftError(err)
    }
}

pub type Error = ValidatorStoreError<SpecificError>;
