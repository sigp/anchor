use types::PublicKey;
pub type ValidatorIndex = usize; // this will be reexported

// Reexports
pub use committee::{Committee, CommitteeID};
pub use keyshare::{Keyshare, SharePublicKey};
pub use operator::{Operator, OperatorStatus, OperatorID};

// modules
mod operator;
mod keyshare;
mod committee;




// Validator that is distributing its duties to a number of Operators
pub struct Validator {
    // The index of the validator
    pub validator_index: ValidatorIndex,
    // Public key of the validator
    pub public_key: PublicKey,
    // The commit for the validator
    pub committee: Committee,
}


