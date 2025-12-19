use database::DatabaseError;

/// Custom execution integration layer errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("sync error: {0}")]
    SyncError(String),

    #[error("invalid event: {0}")]
    InvalidEvent(String),

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("WebSocket error: {0}")]
    WsError(String),

    #[error("failed to decode event: {0}")]
    DecodeError(String),

    #[error("{0}")]
    Misc(String),

    #[error("duplicate entry: {0}")]
    Duplicate(String),

    #[error("database error: {0}")]
    Database(#[from] DatabaseError),

    #[error("database operation failed: {0}")]
    DatabaseOperation(String),
}

impl From<rusqlite::Error> for ExecutionError {
    fn from(error: rusqlite::Error) -> Self {
        ExecutionError::Database(DatabaseError::from(error))
    }
}

impl ExecutionError {
    /// Returns true if this error is critical and the process should crash.
    pub fn is_critical(&self) -> bool {
        match self {
            // Only crash on database system failures, not validation errors
            ExecutionError::Database(db_err) => matches!(
                db_err,
                DatabaseError::SQLError(_)
                    | DatabaseError::SQLPoolError(_)
                    | DatabaseError::IOError(_)
            ),
            ExecutionError::DatabaseOperation(_) | ExecutionError::SyncError(_) => true,
            _ => false,
        }
    }
}
