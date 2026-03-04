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
    #[error("Misc error: {0}")]
    Misc(String),
    #[error("Duplicate: {0}")]
    Duplicate(String),
    #[error("Database error: {0}")]
    Database(String),
}
