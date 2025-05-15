use types::PublicKeyBytes;

use crate::{ClusterId, OperatorId};

// Length of an encrypted key
pub const ENCRYPTED_KEY_LENGTH: usize = 256;

/// One of N shares of a split validator key.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Share {
    /// Public Key of the validator
    pub validator_pubkey: PublicKeyBytes,
    /// Operator this share belongs to
    pub operator_id: OperatorId,
    /// Cluster the operator who owns this share belongs to
    pub cluster_id: ClusterId,
    /// The public key of this Share
    pub share_pubkey: PublicKeyBytes,
    /// The encrypted private key of the share
    pub encrypted_private_key: [u8; ENCRYPTED_KEY_LENGTH],
}
