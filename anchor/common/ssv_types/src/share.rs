use crate::{ClusterId, OperatorId};
use types::PublicKey;

/// One of N shares of a split validator key.
#[derive(Debug, Clone)]
pub struct Share {
    /// Public Key of the validator
    pub validator_pubkey: PublicKey,
    /// Operator this share belongs to
    pub operator_id: OperatorId,
    /// Cluster the operator who owns this share belongs to
    pub cluster_id: ClusterId,
    /// The public key of this Share
    pub share_pubkey: PublicKey,
    /// The encrypted private key of the share
    pub encrypted_private_key: [u8; 256],
}
