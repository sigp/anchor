/// Error associated with Config building.
#[derive(Debug, Clone)]
pub enum ConfigBuilderError {
    /// No participants were specified
    NoParticipants,
    /// At least one round must be done
    ZeroMaxRounds,
    /// Starting round exceeds maximum rounds
    ExceedingStartingRound,
    /// Quorum must be in \[2f+1, participants-f\]
    InvalidQuorumSize,
    /// Operator ID must be specified
    MissingOperatorId,
    /// Operator ID must be contained in participants
    OperatorNotParticipant,
    /// Instance Height must be specified
    MissingInstanceHeight,
}

impl std::error::Error for ConfigBuilderError {}

impl std::fmt::Display for ConfigBuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NoParticipants => {
                write!(f, "No participants were specified")
            }
            Self::ZeroMaxRounds => {
                write!(f, "At least one round must be done")
            }
            Self::ExceedingStartingRound => {
                write!(f, "Starting round exceeds maximum rounds")
            }
            Self::InvalidQuorumSize => {
                write!(f, "Quorum must be in [2f+1, participants-f]")
            }
            Self::MissingOperatorId => {
                write!(f, "Operator ID must be specified")
            }
            Self::OperatorNotParticipant => {
                write!(f, "Operator ID must be contained in participants")
            }
            Self::MissingInstanceHeight => {
                write!(f, "Instance height must be specified")
            }
        }
    }
}
