use bls::PublicKeyBytes;
use bls_lagrange::{KeyId, SharePublicKeyError};
use thiserror::Error;

use crate::{ClusterId, OperatorId, is_valid_committee_size, quorum_size};

// Length of an encrypted key
pub const ENCRYPTED_KEY_LENGTH: usize = 256;

/// Why a validator's share public keys were rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShareValidationError {
    #[error("invalid committee size {0}")]
    InvalidCommitteeSize(usize),
    #[error("operator id {0} cannot be used as a key id")]
    InvalidOperatorId(u64),
    #[error("share public keys do not interpolate to the validator public key: {0:?}")]
    PublicKeys(SharePublicKeyError),
}

/// Checks that `shares` (operator id and share public key) are evaluations of one polynomial of
/// degree exactly quorum-1 whose value at zero is the validator public key, matching go-ssv's
/// `ValidateSharePublicKeys`. Every quorum of operators can then reconstruct signatures for the
/// validator, and no smaller set can. The shares are checked in operator id order, so the error
/// names the same operator whatever order the caller passes.
pub fn validate_share_public_keys(
    validator_pubkey: &PublicKeyBytes,
    shares: &[(OperatorId, PublicKeyBytes)],
) -> Result<(), ShareValidationError> {
    let committee_size = shares.len();
    if !is_valid_committee_size(committee_size) {
        return Err(ShareValidationError::InvalidCommitteeSize(committee_size));
    }

    let mut shares = shares
        .iter()
        .map(|(operator_id, pubkey)| {
            KeyId::try_from(operator_id.0)
                .map(|id| (id, *pubkey))
                .map_err(|_| ShareValidationError::InvalidOperatorId(operator_id.0))
        })
        .collect::<Result<Vec<_>, _>>()?;
    shares.sort_unstable_by_key(|(id, _)| u64::from(id.clone()));

    bls_lagrange::validate_share_public_keys(validator_pubkey, &shares, quorum_size(committee_size))
        .map_err(ShareValidationError::PublicKeys)
}

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
