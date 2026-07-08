use signature_collector::CollectionError;

use crate::{Error, SpecificError};

/// Outcome of a block signing attempt, used for metrics labeling.
pub fn outcome_from_result<T>(result: &Result<T, Error>) -> &'static str {
    match result {
        Ok(_) => "success",
        Err(_) => "failed",
    }
}

pub mod checkpoints {
    pub const RANDAO_REVEAL_ENTERED: &str = "randao_reveal_entered";
    pub const RANDAO_REVEAL_COMPLETED: &str = "randao_reveal_completed";
    pub const RANDAO_REVEAL_FAILED: &str = "randao_reveal_failed";
    pub const DUTY_ENTRY: &str = "duty_entry";
    pub const PRE_CONSENSUS_HANDOFF: &str = "pre_consensus_handoff";
    pub const CONSENSUS_DECIDED: &str = "consensus_decided";
    pub const BLOCK_SIGNED: &str = "block_signed";
    pub const PUBLISH_BLOCK: &str = "publish_block";
    pub const DUTY_COMPLETED: &str = "duty_completed";
    pub const DUTY_FAILED: &str = "duty_failed";
}

/// Telemetry classification of `collect_signature` failures.
///
/// Shared by every single-validator signing path (PTC, ProposerPreferences, ...): the
/// classification depends only on the generic `collect_signature` error, never on the role, so
/// each caller maps these classes onto its own log message and reconstruction-failure metric.
///
/// Returned as an enum rather than a label string because the class drives two independent
/// effects in the caller: the log level and whether a reconstruction-failure metric is
/// incremented at all.
pub enum CollectionFailureClass {
    /// The committee never reached the partial signature threshold. This surfaces as
    /// `QueueClosedError` because the collector is evicted after
    /// `SIGNATURE_COLLECTOR_RETAIN_SLOTS`, dropping the result channel while we await it, so it
    /// cannot be distinguished from a genuine channel close.
    NoSignature,
    /// Local infrastructure failed while collecting or reconstructing the signature.
    Infra,
    /// The failure happened before signature collection started (unknown pubkey, threshold
    /// arithmetic, key share decryption, missing index).
    NonCollection,
}

pub fn classify_collection_failure(error: &Error) -> CollectionFailureClass {
    match error {
        // The inner match is deliberately wildcard-free so a future `CollectionError` variant
        // forces a conscious classification decision here at compile time.
        Error::SpecificError(SpecificError::SignatureCollectionFailed(collection_error)) => {
            match collection_error {
                CollectionError::QueueClosedError | CollectionError::CollectionTimeout => {
                    CollectionFailureClass::NoSignature
                }
                CollectionError::QueueFullError
                | CollectionError::OwnOperatorIdUnknown
                | CollectionError::EmptySignature
                | CollectionError::RecoverError(_) => CollectionFailureClass::Infra,
            }
        }
        _ => CollectionFailureClass::NonCollection,
    }
}

/// Common reasons for block signing failures.
pub fn failure_reason(error: &Error) -> &'static str {
    match error {
        Error::SpecificError(SpecificError::ArithError(_)) => "arith_error",
        Error::SpecificError(SpecificError::ClusterLiquidated) => "cluster_liquidated",
        Error::SpecificError(SpecificError::DataTooLarge(_)) => "data_too_large",
        Error::SpecificError(SpecificError::InvalidQbftData(_)) => "invalid_qbft_data",
        Error::SpecificError(SpecificError::KeyShareDecryptionFailed) => {
            "key_share_decryption_failed"
        }
        Error::SpecificError(SpecificError::MissingIndex) => "missing_index",
        Error::SpecificError(SpecificError::NoDataAgreed) => "no_data_agreed",
        Error::SpecificError(SpecificError::NotSynced) => "not_synced",
        Error::SpecificError(SpecificError::QbftError(_)) => "qbft_error",
        Error::SpecificError(SpecificError::SignatureCollectionFailed(_)) => {
            "signature_collection_failed"
        }
        Error::SpecificError(SpecificError::SlotClock) => "slot_clock",
        Error::SpecificError(SpecificError::Timeout) => "timeout",
        Error::SpecificError(SpecificError::ValidatorClusterMismatch { .. }) => {
            "validator_cluster_mismatch"
        }
        Error::Slashable(_) => "slashable",
        Error::SameData => "same_data",
        Error::SpecificError(_) => "other_specific_error",
        Error::UnknownPubkey(_) => "unknown_pubkey",
        _ => "other_error",
    }
}
