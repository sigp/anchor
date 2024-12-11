use types::{PublicKey, SecretKey};

/// One of N shares of a split validator key.
#[derive(Clone)]
pub struct Share {
    /// The public key of this Share
    pub share_pubkey: PublicKey,
    /// The encrypted private key of the share
    pub encrypted_private_key: SecretKey,
}
