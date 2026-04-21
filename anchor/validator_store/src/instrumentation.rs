use crate::{Error, SpecificError};

/// Outcome of a block signing attempt, used for metrics labeling.
#[derive(Debug)]
pub enum SignBlockOutcome {
    Success,
    Timeout,
    Failed,
}

impl SignBlockOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Timeout => "timeout",
            Self::Failed => "error",
        }
    }
    pub fn from_result<T>(result: &Result<T, Error>) -> Self {
        match result {
            Ok(_) => Self::Success,
            Err(Error::SpecificError(SpecificError::Timeout)) => Self::Timeout,
            Err(_) => Self::Failed,
        }
    }
}

#[derive(Debug)]
pub enum BlockSigningCheckpoints {
    DutyEntry,
    PreConsensusHandoff,
    ConsensusDecided,
    BlockSigned,
    PublishConsensus,
}

impl BlockSigningCheckpoints {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DutyEntry => "duty_entry",
            Self::PreConsensusHandoff => "qbft_start",
            Self::ConsensusDecided => "consensus_decided",
            Self::BlockSigned => "block_signed",
            Self::PublishConsensus => "publish_consensus",
        }
    }
}
