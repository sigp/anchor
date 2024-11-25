type ValidatorIndex = usize; // this will come in from types
use crate::OperatorID;

// Unique identifier for a committee
pub type CommitteeID = u64;

// A committee of operators
pub struct Committee {
    // Validator this committee corresponds with
    pub validator_index: ValidatorIndex,
    // Identification for the committee
    pub id: CommitteeID,
    // All of the operators in the committee
    pub operators: Vec<OperatorID>,
    // How many operators are needed for consensus
    pub threadhold: u64,
    // Is this committee active
    pub active: bool,
}
