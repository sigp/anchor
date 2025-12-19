use std::io::ErrorKind;

/// Database operation errors
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("already present: {0}")]
    AlreadyPresent(String),

    #[error("IO error: {0}")]
    IOError(ErrorKind),

    #[error("SQL error: {0}")]
    SQLError(#[from] rusqlite::Error),

    #[error("connection pool error: {0}")]
    SQLPoolError(#[from] r2d2::Error),
}
