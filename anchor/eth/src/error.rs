use thiserror::Error;

/// Errors from the execution-layer integration.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("Sync error: {0}")]
    SyncError(String),
    #[error("Invalid event: {0}")]
    InvalidEvent(String),
    #[error("RPC error: {0}")]
    RpcError(String),
    #[error("WebSocket error: {0}")]
    WsError(String),
    #[error("Decode error: {0}")]
    DecodeError(#[from] alloy::sol_types::Error),
    #[error("Skipped event: {0}")]
    SkippedEvent(String),
    #[error("Duplicate: {0}")]
    Duplicate(String),
    #[error("Database error: {0}")]
    Database(String),
    #[error("Exit processor unavailable: {0}")]
    ExitProcessorUnavailable(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogErrorDisposition {
    SkipMalformed,
    SkipExpected,
    AbortBatch,
}

impl ExecutionError {
    pub fn log_disposition(&self) -> LogErrorDisposition {
        match self {
            Self::InvalidEvent(_) | Self::DecodeError(_) | Self::Duplicate(_) => {
                LogErrorDisposition::SkipMalformed
            }
            Self::SkippedEvent(_) => LogErrorDisposition::SkipExpected,
            Self::SyncError(_)
            | Self::RpcError(_)
            | Self::WsError(_)
            | Self::Database(_)
            | Self::ExitProcessorUnavailable(_) => LogErrorDisposition::AbortBatch,
        }
    }
}
