use types::PublicKey;
use crate::{Keyshare, Committee};

// Unique idetifier for an Operator
pub type OperatorID = usize;

// Current operational status of an operator
pub enum OperatorStatus {
    Active,
    Inactive,
}

// Client responsible for maintaining the overall health of the network.
pub struct Operator {
    // ID to uniquely identify this operator
    pub id: OperatorID,
    // Public key of the operator
    pub public_key: PublicKey,
    // All of the validator shares this operator is reponsible for
    pub keyshares: Vec<Keyshare>,
    // All of the committees this operator is in
    pub committees: Vec<Committee>,
    // Operation status of the operator
    pub status: OperatorStatus,
}
