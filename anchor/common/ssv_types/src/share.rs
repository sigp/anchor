use derive_more::{Deref, From};
use types::{Address, Graffiti, PublicKey};

/// Index of the validator in the validator registry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct ValidatorIndex(pub usize);

/// One of N shares of a split validator key.
#[derive(Debug, Clone)]
pub struct Share {
    /// The public key of this Share
    pub share_pubkey: PublicKey,
    /// Metadata about the Validator this Share corresponds to
    pub validator_metadata: ValidatorMetadata,
}

/// General Metadata about a Validator
#[derive(Debug, Clone)]
pub struct ValidatorMetadata {
    /// Index of the validator
    pub validator_index: ValidatorIndex,
    /// Public key of the validator
    pub validator_pubkey: PublicKey,
    /// Eth1 fee address
    pub fee_recipient: Address,
    /// Graffiti
    pub graffiti: Graffiti,
    /// The owner of the validator
    pub owner: Address,
}
