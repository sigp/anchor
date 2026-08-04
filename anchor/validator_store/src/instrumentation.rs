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
    /// Reveal reconstructed, before any proposer delay. [`RANDAO_REVEAL_COMPLETED`] fires after it
    /// and so includes the wait.
    pub const RANDAO_REVEAL_RECONSTRUCTED: &str = "randao_reveal_reconstructed";
    pub const PROPOSER_DELAY_APPLIED: &str = "proposer_delay_applied";
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
