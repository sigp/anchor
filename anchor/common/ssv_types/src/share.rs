use crate::OperatorId;
use derive_more::{Deref, From};
use std::time::SystemTime;
use types::{Address, Domain, Graffiti, PublicKey};

/// Index of the validator in the validator registry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct ValidatorIndex(usize);

/// Share of a key that a operator owns and its accompanying metadata.
#[derive(Debug, Clone)]
pub struct SSVShare {
    // A single share of a validator private key.
    pub share: Share,
    // Miscellaneous metadata relevant to the share
    pub metadata: Metadata,
}

/// One of N shares of a split validator key.
#[derive(Debug, Clone)]
pub struct Share {
    /// Index of the validator
    pub validator_index: ValidatorIndex,
    /// Public key of the validator
    pub validator_pubkey: PublicKey,
    /// Public key for this portion of the share
    pub share_public_key: PublicKey,
    /// All committee members that contain a sibling share
    pub committee: Vec<ShareMember>,
    /// Identifies the context/purpose of signature
    pub domain_type: Domain,
    /// Eth1 fee address
    pub fee_recipient: Address,
    /// Graffiti
    pub graffiti: Graffiti,
}

/// A operator who holds a portion of the share.
#[derive(Debug, Clone)]
pub struct ShareMember {
    /// Unique identifier for the operator
    pub operator: OperatorId,
    /// The public key for this members share
    pub share_public_key: PublicKey,
}

/// General metadata.
#[derive(Debug, Clone)]
pub struct Metadata {
    /// The owner of the validator
    pub owner: Address,
    /// Is the committee this share is a part of currently liquidated
    pub liquidated: bool,
    /// Track the last time the metadata was updated.
    pub last_updated: SystemTime,
}
