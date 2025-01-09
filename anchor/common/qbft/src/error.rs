/// Error associated with Config building.
// TODO: Remove this allow
#[derive(Debug)]
pub enum ConfigBuilderError {
    /// Quorum size too small
    QuorumSizeTooSmall,
}

impl std::error::Error for ConfigBuilderError {}

impl std::fmt::Display for ConfigBuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::QuorumSizeTooSmall => {
                write!(f, "Quorum size too small")
            }
        }
    }
}
