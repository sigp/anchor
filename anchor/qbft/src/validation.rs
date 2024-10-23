//! Validation for data function

use crate::types::ConsensusData;

/// The list of possible validation errors that can occur
#[derive(Debug)]
pub enum ValidationError {
    Invalid,
}

/// Data that has been validated by our validation function.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct ValidatedData<D> {
    pub data: D,
}

/// This verifies the data is correct an appropriate to use for consensus.
pub fn validate_data<D>(data: D) -> Result<ValidatedData<D>, ValidationError> {
    Ok(ValidatedData { data })
}

// Validates consensus data
pub fn validate_consensus_data<D>(
    consensus_data: ConsensusData<D>,
) -> Result<ConsensusData<ValidatedData<D>>, ValidationError> {
    let round = consensus_data.round;
    let validated_data = validate_data(consensus_data.data)?;
    Ok(ConsensusData {
        round,
        data: validated_data,
    })
}
