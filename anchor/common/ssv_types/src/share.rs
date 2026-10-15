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

#[cfg(test)]
mod tests {
    use bls::{SECRET_KEY_BYTES_LEN, SecretKey};
    use rand::{RngCore, SeedableRng, rngs::StdRng, seq::SliceRandom};

    use super::*;

    // ==================== validate_share_public_keys helpers ====================

    /// Committee sizes go-ssv's `TestValidateSharePublicKeys` expects the wrapper to reject
    /// before looking at any key: everything outside `{4, 7, 10, 13}` up to the largest checked.
    const NON_CANONICAL_COMMITTEE_SIZES: [usize; 8] = [0, 1, 2, 3, 5, 6, 14, 16];

    const VALID_COMMITTEE_SIZE: usize = 4;

    /// A secret key derived from `rng`, so each test is reproducible. The top byte is masked so
    /// the big-endian scalar is below the BLS12-381 group order, as `deserialize` requires.
    fn seeded_secret_key(rng: &mut StdRng) -> SecretKey {
        let mut bytes = [0u8; SECRET_KEY_BYTES_LEN];
        rng.fill_bytes(&mut bytes);
        bytes[0] &= 0x3f;
        SecretKey::deserialize(&bytes).unwrap()
    }

    fn operator_ids(committee_size: usize) -> Vec<OperatorId> {
        (1..=committee_size as u64).map(OperatorId).collect()
    }

    /// Share public keys for `ids` on a polynomial of degree `quorum - 1` with `key` as its free
    /// coefficient, in id order. Below a quorum of 2 (committees of up to three) the polynomial
    /// is constant, so every share is the validator key itself, as in go-ssv's fixture.
    fn split_share_public_keys(
        rng: &mut StdRng,
        key: &SecretKey,
        ids: &[OperatorId],
    ) -> Vec<(OperatorId, PublicKeyBytes)> {
        let threshold = quorum_size(ids.len());
        if threshold < 2 {
            let validator_pubkey = PublicKeyBytes::from(key.public_key());
            return ids.iter().map(|id| (*id, validator_pubkey)).collect();
        }
        let key_ids = ids.iter().map(|id| KeyId::try_from(id.0).unwrap());
        bls_lagrange::split_with_rng(key, threshold as u64, key_ids, rng)
            .unwrap()
            .into_iter()
            .map(|(id, secret)| {
                (
                    OperatorId(u64::from(id)),
                    PublicKeyBytes::from(secret.public_key()),
                )
            })
            .collect()
    }

    /// A random validator public key with a share set that interpolates to it at the would-be
    /// quorum for operators `1..=committee_size`.
    fn valid_committee(
        rng: &mut StdRng,
        committee_size: usize,
    ) -> (PublicKeyBytes, Vec<(OperatorId, PublicKeyBytes)>) {
        let key = seeded_secret_key(rng);
        let shares = split_share_public_keys(rng, &key, &operator_ids(committee_size));
        (PublicKeyBytes::from(key.public_key()), shares)
    }

    // ==================== validate_share_public_keys: committee shape ====================

    /// Mirrors go-ssv "invalid committee size": the size check runs first, so even a share set
    /// consistent with the would-be quorum is rejected for its size alone.
    #[test]
    fn test_validate_share_public_keys_rejects_non_canonical_committee_sizes() {
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000020);

        for committee_size in NON_CANONICAL_COMMITTEE_SIZES {
            // Arrange
            let (validator_pubkey, shares) = valid_committee(rng, committee_size);

            // Act
            let result = validate_share_public_keys(&validator_pubkey, &shares);

            // Assert
            assert_eq!(
                result,
                Err(ShareValidationError::InvalidCommitteeSize(committee_size)),
                "committee of {committee_size} must be rejected"
            );
        }
    }

    #[test]
    fn test_validate_share_public_keys_rejects_operator_id_zero() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000021);
        let (validator_pubkey, mut shares) = valid_committee(rng, VALID_COMMITTEE_SIZE);
        shares[0].0 = OperatorId(0);

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares);

        // Assert
        assert_eq!(result, Err(ShareValidationError::InvalidOperatorId(0)));
    }

    // ==================== validate_share_public_keys: share order ====================

    /// The wrapper sorts by operator id, so the caller's order must not matter.
    #[test]
    fn test_validate_share_public_keys_accepts_shares_in_any_order() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000022);
        let (validator_pubkey, shares) = valid_committee(rng, VALID_COMMITTEE_SIZE);
        let mut reversed = shares.clone();
        reversed.reverse();
        let mut shuffled = shares;
        shuffled.shuffle(rng);

        // Act
        let reversed_result = validate_share_public_keys(&validator_pubkey, &reversed);
        let shuffled_result = validate_share_public_keys(&validator_pubkey, &shuffled);

        // Assert
        assert_eq!(reversed_result, Ok(()));
        assert_eq!(shuffled_result, Ok(()));
    }

    /// Mirrors go-ssv "shares out of order": swapping two share public keys while the ids stay
    /// in place presents those shares at the wrong ids, which sorting cannot repair.
    #[test]
    fn test_validate_share_public_keys_rejects_swapped_share_public_keys() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000023);
        let (validator_pubkey, mut shares) = valid_committee(rng, VALID_COMMITTEE_SIZE);
        let last = shares.len() - 1;
        let first_pubkey = shares[0].1;
        shares[0].1 = shares[last].1;
        shares[last].1 = first_pubkey;

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares);

        // Assert
        assert!(
            matches!(result, Err(ShareValidationError::PublicKeys(_))),
            "swapped share public keys must be rejected, got {result:?}"
        );
    }
}
