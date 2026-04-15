use crate::errors::{Error, SpecificError};

#[derive(Debug, Clone, Copy)]
/// Instrumentation taxonomy reported outcomes of signing a block.
pub enum SignBlockOutcome {
    Success,
    Timeout,
    Error,
}

#[expect(dead_code)]
impl SignBlockOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }
    pub fn from_result<T>(result: &Result<T, Error>) -> Self {
        match result {
            Ok(_) => Self::Success,
            Err(Error::SpecificError(SpecificError::Timeout)) => Self::Timeout,
            Err(_) => Self::Error,
        }
    }
}
