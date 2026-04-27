use derive_more::Display;

use crate::{Error, SpecificError};

/// Outcome of a block signing attempt, used for metrics labeling.
#[derive(Debug, Display)]
pub enum SignBlockOutcome {
    #[display("success")]
    Success,
    #[display("timeout")]
    Timeout,
    #[display("error")]
    Failed,
}

impl SignBlockOutcome {
    pub fn from_result<T>(result: &Result<T, Error>) -> Self {
        match result {
            Ok(_) => Self::Success,
            Err(Error::SpecificError(SpecificError::Timeout)) => Self::Timeout,
            Err(_) => Self::Failed,
        }
    }
}

pub mod checkpoints {
    pub const DUTY_ENTRY: &str = "duty_entry";
    pub const PRE_CONSENSUS_HANDOFF: &str = "pre_consensus_handoff";
    pub const CONSENSUS_DECIDED: &str = "consensus_decided";
    pub const BLOCK_SIGNED: &str = "block_signed";
    pub const PUBLISH_BLOCK: &str = "publish_block";
    pub const DUTY_COMPLETED: &str = "duty_completed";
    pub const DUTY_FAILED: &str = "duty_failed";
}
