use multi_index_map::MultiIndexMap;
use types::{Address, PublicKeyBytes};

use crate::{ClusterId, CommitteeId, OperatorId};

// Length of an encrypted key
pub const ENCRYPTED_KEY_LENGTH: usize = 256;

/// One of N shares of a split validator key.
#[derive(Debug, Clone, Eq, PartialEq, Hash, MultiIndexMap)]
pub struct Share {
    /// Public Key of the validator
    #[multi_index(hashed_unique)]
    pub validator_pubkey: PublicKeyBytes,
    /// Operator this share belongs to
    pub operator_id: OperatorId,
    /// Cluster the operator who owns this share belongs to
    #[multi_index(hashed_non_unique)]
    pub cluster_id: ClusterId,
    /// The public key of this Share
    pub share_pubkey: PublicKeyBytes,
    /// The encrypted private key of the share
    pub encrypted_private_key: [u8; ENCRYPTED_KEY_LENGTH],
    /// Owner address - computed field from cluster
    pub owner: Address,
    /// Committee ID - computed field from cluster members
    #[multi_index(hashed_non_unique)]
    pub committee_id: CommitteeId,
}

impl Share {
    /// Create a new Share
    pub fn new(
        validator_pubkey: PublicKeyBytes,
        operator_id: OperatorId,
        cluster_id: ClusterId,
        share_pubkey: PublicKeyBytes,
        encrypted_private_key: [u8; ENCRYPTED_KEY_LENGTH],
        owner: Address,
        committee_id: CommitteeId,
    ) -> Self {
        Self {
            validator_pubkey,
            operator_id,
            cluster_id,
            share_pubkey,
            encrypted_private_key,
            owner,
            committee_id,
        }
    }
}
