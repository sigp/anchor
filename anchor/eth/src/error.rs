use std::fmt::Display;

// Custom execution integration layer errors
#[derive(Debug)]
pub enum ExecutionError {
    SyncError(String),
    InvalidEvent(String),
    RpcError(String),
    DecodeError(String),
    Misc(String),
    Duplicate(String),
    Database(String),
}

impl Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
