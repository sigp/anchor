use types::{Address, Domain, Graffiti, PublicKey}; // ValidatorIndex
type ValidatorIndex = usize; // this will come from types
use crate::CommitteeID;
use std::time::SystemTime;


// Share of a key that a operator owns and accompanying metadata
#[derive(Debug, Clone)]
pub struct SSVShare {
    pub share: Share,
    pub metadata: Metadata,
}

// One of N shares of a split validator key
#[derive(Debug, Clone)]
pub struct Share {
    // Index of the validator
    pub validator_index: ValidatorIndex,
    // Public key of the validator
    pub validator_pubkey: PublicKey,
    // Public key for this portion of the share
    pub share_public_key: PublicKey,
    // All committee members that contain a sibling share
    pub committee: Vec<ShareMember>,
    // Identifies the context/purpose of signature
    pub domain_type: Domain,
    // Eth1 fee address
    pub fee_recipient: Address,
    // Graffiti
    pub graffiti: Graffiti,
}

// A operator who also holds a portion of this share
// A less descriptive reference to a CommitteeMember
#[derive(Debug, Clone)]
pub struct ShareMember {
    // Unique identifier for the operator
    pub operator: CommitteeID,
    // The public key for this members share
    pub share_public_key: PublicKey,
}

// Share metadata
#[derive(Debug, Clone)]
pub struct Metadata {
    // The owner of the validator
    pub owner: Address,
    // Is the commitee this share part of currently liquidated
    pub liquidated: bool,
    // Track the last time the metadata was updated. todo!() this or chrono
    pub last_updated: SystemTime,
}
