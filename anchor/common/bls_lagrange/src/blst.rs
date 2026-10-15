// from https://github.com/herumi/mcl/blob/3462cf0983bffb703a6e9f4623e47a26ec6e7fe5/include/mcl/lagrange.hpp
use std::{
    iter::{once, repeat_with},
    mem,
    num::NonZeroU64,
    ptr,
};

use bls::{PublicKeyBytes, Signature};
use blst::{min_pk::SecretKey, *};
use rand::prelude::*;

use crate::{Error, random_key};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyId {
    num: u64,
    // note: while blst_scalar is also used for bls keys, the scalars used in key ids are NOT
    // secret
    scalar: blst_scalar,
}

impl TryFrom<u64> for KeyId {
    type Error = Error;

    fn try_from(value: u64) -> Result<Self, Error> {
        // The key id needs to be non-zero, as f(0) is the secret we are sharing.
        if value != 0 {
            unsafe {
                let mut id = blst_scalar::default();
                let value_le_bytes = value.to_le_bytes();
                blst_scalar_from_le_bytes(&mut id, value_le_bytes.as_ptr(), 8);
                Ok(KeyId {
                    num: value,
                    scalar: id,
                })
            }
        } else {
            Err(Error::ZeroId)
        }
    }
}
impl From<NonZeroU64> for KeyId {
    fn from(value: NonZeroU64) -> Self {
        unsafe {
            let mut id = blst_scalar::default();
            let value_le_bytes = value.get().to_le_bytes();
            blst_scalar_from_le_bytes(&mut id, value_le_bytes.as_ptr(), 8);
            KeyId {
                num: value.get(),
                scalar: id,
            }
        }
    }
}

impl From<KeyId> for u64 {
    fn from(value: KeyId) -> Self {
        value.num
    }
}

pub fn split_with_rng(
    key: &bls::SecretKey,
    threshold: u64,
    ids: impl IntoIterator<Item = KeyId>,
    rng: &mut (impl CryptoRng + Rng),
) -> Result<Vec<(KeyId, bls::SecretKey)>, Error> {
    if threshold <= 1 {
        return Err(Error::InvalidThreshold);
    }

    // `bls::SecretKey` contains a blst `SecretKey`, which zeroizes on drop.
    // These are the random coefficients for our polynomial.
    let random_coefficients = repeat_with(|| random_key(rng))
        .take((threshold - 1) as usize)
        .collect::<Result<Vec<_>, _>>()?;

    // This will always have len == threshold, so it's non-empty
    let coefficients = once(key)
        .chain(random_coefficients.iter())
        .map(|key| <&blst_scalar>::from(key.point()))
        .collect::<Vec<_>>();

    unsafe {
        if !blst_sk_check(coefficients[0]) {
            return Err(Error::ZeroKey);
        }
    }

    ids.into_iter()
        .map(|id| unsafe {
            // Compute f(id), which is the secret for the participant with that id.

            let mut y = (*coefficients.last().expect("coefficients is non-empty")).clone();
            // As threshold is 2 or greater, this will do at least one iteration.
            // At the beginning of the first iteration, y is the coefficient of x^threshold.
            // We multiply it by x (=id), and add the coefficient of x^(threshold - 1), until we add
            // x^0.
            // This works because ((c2*x) + c1) * x) + c0 = c0 + c1*x + c2*x^2
            for i in (0..=(threshold - 2)).rev() {
                // "check" refers to checking if the result is 0. "n" is short for "and".
                // The references coerce to pointers, which are allowed to alias in Rust.
                // blst takes care to write to the result pointer only after it is done reading
                // from the input pointers, so it is fine to reuse y here.
                if !blst_sk_mul_n_check(&mut y, &y, &id.scalar) {
                    return Err(Error::ZeroId);
                }
                assert!(blst_sk_add_n_check(&mut y, &y, coefficients[i as usize]));
            }
            // SecretKey is repr(transparent), so the transmute is fine.
            // We pass a reference, and afterward, the SecretKey is dropped, zeroizing it.
            Ok((
                id,
                bls::SecretKey::from_point(&mem::transmute::<blst_scalar, SecretKey>(y)),
            ))
        })
        .collect()
}

pub fn combine_signatures(signatures: &[Signature], ids: &[KeyId]) -> Result<Signature, Error> {
    if signatures.len() < 2 {
        return Err(Error::LessThanTwoSignatures);
    }
    if signatures.len() != ids.len() {
        return Err(Error::NotOneIdPerSignature);
    }

    // We are doing this:
    // https://en.wikipedia.org/wiki/Shamir%27s_secret_sharing#Computationally_efficient_approach
    // We have the signatures (= y) and key ids (= x)
    // We gather all the inner products (big Pi) and later multiply them with their corresponding
    // signature efficiently via `mult`.

    // First, convert all the signatures to the `blst` type.
    // Neither signatures or ids are secret, so we don't care about zeroization.
    let signatures = signatures
        .iter()
        .map(|sig| sig.point().cloned().ok_or(Error::InvalidSignature))
        .collect::<Result<Vec<_>, _>>()?;

    let d = lagrange_coefficients(ids)?;

    // `mult` multiplies the signatures and scalars pairwise, then sums them up.
    Ok(Signature::from_point(mult(&signatures, &d), false))
}

/// Lagrange coefficients at x = 0 for the polynomial through points with the given ids, one per
/// id in input order, concatenated as the 32-byte scalars `blst` multi-scalar multiplication
/// expects. Requires at least one id.
fn lagrange_coefficients(ids: &[KeyId]) -> Result<Vec<u8>, Error> {
    let Some(first) = ids.first() else {
        return Err(Error::InternalError);
    };

    let mut intermediate = blst_scalar::default();

    // "numerator" is the product of all key ids
    let mut numerator = first.scalar.clone();
    unsafe {
        for id in &ids[1..] {
            // Again, false is returned if we multiply by zero.
            if !blst_sk_mul_n_check(&mut numerator, &numerator, &id.scalar) {
                return Err(Error::ZeroId);
            }
        }
    }

    // For some reason, the blst API expects that the scalars are passed as a byte slice.
    let mut d = Vec::with_capacity(ids.len() * 32);
    unsafe {
        for id_i in ids {
            // For performance, we want to "divide" (invert and then multiply) only once per key.
            // Rewriting the product, you can see that the numerator needs to be the product of all
            // key ids, except "id_i". But above we have precomputed the numerator to be ALL key
            // ids. So we start by putting id_i into the denominator to compensate for that.
            let mut denominator = id_i.scalar.clone();
            for id_j in ids.iter() {
                if !ptr::eq(id_i, id_j) {
                    // If we end up having zero here, the user specified the same key more than once
                    if !blst_sk_sub_n_check(&mut intermediate, &id_j.scalar, &id_i.scalar) {
                        return Err(Error::RepeatedId);
                    }
                    // Multiply the difference we computed just now with the current denominator
                    assert!(blst_sk_mul_n_check(
                        &mut denominator,
                        &denominator,
                        &intermediate
                    ));
                }
            }
            // Now, we got a denominator consisting of x_i * (x_0 - x_i) * (x_1 - x_i) * ...
            // By dividing `numerator` with this, we get the inner product, which we store to be
            // later multiplied with the corresponding signature.
            blst_sk_inverse(&mut denominator, &denominator);
            assert!(blst_sk_mul_n_check(
                &mut intermediate,
                &denominator,
                &numerator
            ));
            d.extend(&intermediate.b);
        }
    }

    Ok(d)
}

/// Why [`validate_share_public_keys`] rejected a set of share public keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharePublicKeyError {
    /// `threshold` must be at least 2 and at most the number of shares.
    InvalidThreshold { threshold: usize, shares: usize },
    /// The validator public key is not a valid G1 point.
    InvalidValidatorPublicKey,
    /// The share public key with this id is not a valid G1 point.
    InvalidSharePublicKey { id: u64 },
    /// The ids cannot serve as interpolation points (zero or repeated).
    Ids(Error),
    /// The first `threshold - 1` shares alone already reconstruct the validator public key, so
    /// the polynomial degree is below `threshold - 1` and fewer than `threshold` shares can sign.
    DegreeBelowThreshold,
    /// The share with this id does not lie on the polynomial defined by the base shares and the
    /// validator public key.
    NotReconstructed { id: u64 },
}

impl From<Error> for SharePublicKeyError {
    fn from(error: Error) -> Self {
        Self::Ids(error)
    }
}

/// Checks that `shares` are evaluations, at their ids, of one polynomial of degree exactly
/// `threshold - 1` whose value at zero is `validator_pubkey`, so that any `threshold` shares, and
/// no fewer, reconstruct signatures for the validator key.
///
/// Mirrors go-ssv's `ValidateSharePublicKeys`: the first `threshold - 1` shares together with
/// `(0, validator_pubkey)` define a polynomial of degree at most `threshold - 1`. The base alone
/// must not already reconstruct the key (otherwise the degree is lower), and the base plus each
/// remaining share must, which puts every share on that polynomial.
pub fn validate_share_public_keys(
    validator_pubkey: &PublicKeyBytes,
    shares: &[(KeyId, PublicKeyBytes)],
    threshold: usize,
) -> Result<(), SharePublicKeyError> {
    if threshold < 2 || threshold > shares.len() {
        return Err(SharePublicKeyError::InvalidThreshold {
            threshold,
            shares: shares.len(),
        });
    }

    // `key_validate` also rejects the point at infinity and points outside the G1 subgroup.
    let expected = min_pk::PublicKey::key_validate(validator_pubkey.as_serialized())
        .map_err(|_| SharePublicKeyError::InvalidValidatorPublicKey)?;
    let (ids, pubkeys): (Vec<KeyId>, Vec<min_pk::PublicKey>) = shares
        .iter()
        .map(|(id, pubkey)| {
            min_pk::PublicKey::key_validate(pubkey.as_serialized())
                .map(|pubkey| (id.clone(), pubkey))
                .map_err(|_| SharePublicKeyError::InvalidSharePublicKey { id: id.num })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip();

    let base = threshold - 1;
    if interpolate_public_key(&pubkeys[..base], &ids[..base])? == expected {
        return Err(SharePublicKeyError::DegreeBelowThreshold);
    }

    let mut subset_ids = ids[..base].to_vec();
    let mut subset_pubkeys = pubkeys[..base].to_vec();
    for (id, pubkey) in ids[base..].iter().zip(&pubkeys[base..]) {
        subset_ids.push(id.clone());
        subset_pubkeys.push(*pubkey);
        let recovered = interpolate_public_key(&subset_pubkeys, &subset_ids)?;
        subset_ids.pop();
        subset_pubkeys.pop();
        if recovered != expected {
            return Err(SharePublicKeyError::NotReconstructed { id: id.num });
        }
    }

    Ok(())
}

/// Evaluates at zero the polynomial through the given public keys: the G1 counterpart of
/// [`combine_signatures`].
fn interpolate_public_key(
    pubkeys: &[min_pk::PublicKey],
    ids: &[KeyId],
) -> Result<min_pk::PublicKey, Error> {
    let d = lagrange_coefficients(ids)?;
    Ok(mult_public_keys(pubkeys, &d))
}

#[cfg(not(feature = "blst_single_thread"))]
fn mult_public_keys(pubkeys: &[min_pk::PublicKey], d: &[u8]) -> min_pk::PublicKey {
    pubkeys.mult(d, 255).to_public_key()
}

#[cfg(feature = "blst_single_thread")]
fn mult_public_keys(pubkeys: &[min_pk::PublicKey], d: &[u8]) -> min_pk::PublicKey {
    let mut ret = blst_p1::default();
    let mut ret_affine = blst_p1_affine::default();
    let p: [*const blst_p1_affine; 2] = [<&blst_p1_affine>::from(&pubkeys[0]), std::ptr::null()];
    let s: [*const u8; 2] = [&d[0], std::ptr::null()];
    unsafe {
        let mut scratch = vec![0; blst_p1s_mult_pippenger_scratch_sizeof(pubkeys.len()) / 8];
        blst_p1s_mult_pippenger(
            &mut ret,
            &p[0],
            pubkeys.len(),
            &s[0],
            255,
            scratch.as_mut_ptr(),
        );
        blst_p1_to_affine(&mut ret_affine, &ret)
    }
    ret_affine.into()
}

#[cfg(not(feature = "blst_single_thread"))]
fn mult(signatures: &[min_pk::Signature], d: &[u8]) -> min_pk::Signature {
    signatures.mult(d, 255).to_signature()
}

#[cfg(feature = "blst_single_thread")]
fn mult(signatures: &[min_pk::Signature], d: &[u8]) -> min_pk::Signature {
    let mut ret = blst_p2::default();
    let mut ret_affine = blst_p2_affine::default();
    let p: [*const blst_p2_affine; 2] = [<&blst_p2_affine>::from(&signatures[0]), std::ptr::null()];
    let s: [*const u8; 2] = [&d[0], std::ptr::null()];
    unsafe {
        let mut scratch = vec![0; blst_p2s_mult_pippenger_scratch_sizeof(signatures.len()) / 8];
        blst_p2s_mult_pippenger(
            &mut ret,
            &p[0],
            signatures.len(),
            &s[0],
            255,
            scratch.as_mut_ptr(),
        );
        blst_p2_to_affine(&mut ret_affine, &ret)
    }
    ret_affine.into()
}

#[cfg(test)]
mod tests {
    use bls::INFINITY_PUBLIC_KEY;

    use super::*;

    #[test]
    fn test_key_id_from_u64() {
        let mut scalar = blst_scalar::default();

        let mut arr = [0u8; 128];
        StdRng::seed_from_u64(0x1234565EED << 11).fill_bytes(&mut arr);

        for i in 0..(arr.len() - 8) {
            assert_eq!(
                // passing the u64 by value...
                &crate::blst::KeyId::try_from(u64::from_le_bytes(
                    arr[i..i + 8].try_into().unwrap()
                ))
                .unwrap()
                .scalar,
                // ...should return the same as pointing to our array
                unsafe {
                    blst_scalar_from_le_bytes(&mut scalar, &arr[i], 8);
                    &scalar
                }
            );
        }
    }

    // ==================== validate_share_public_keys helpers ====================

    /// Committee sizes with the quorum threshold formula used by SSV clusters.
    const COMMITTEE_SIZES: [usize; 4] = [4, 7, 10, 13];

    /// Non-contiguous operator ids used to check that validation does not depend on ids being
    /// `1..=n`.
    const SPARSE_IDS: [u64; 4] = [7, 42, 13, 1000];

    /// The committees go-ssv's `TestValidateSharePublicKeys` runs every case against: one of
    /// each canonical size, mixing contiguous and sparse ids. Mirrored here so both
    /// implementations are exercised on the same inputs.
    const GO_SSV_COMMITTEES: [&[u64]; 4] = [
        &[1, 2, 3, 4],
        &[5, 17, 23, 42, 100, 101, 999],
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        &[11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23],
    ];

    /// SSV quorum threshold: `2f + 1` with `f = (n - 1) / 3`.
    fn quorum_threshold(committee_size: usize) -> usize {
        2 * ((committee_size - 1) / 3) + 1
    }

    fn key_ids(ids: impl IntoIterator<Item = u64>) -> Vec<KeyId> {
        ids.into_iter()
            .map(|id| KeyId::try_from(id).unwrap())
            .collect()
    }

    /// Splits `key` for the given ids and returns the serialized share public keys in id order.
    fn split_share_public_keys(
        rng: &mut StdRng,
        key: &bls::SecretKey,
        threshold: usize,
        ids: Vec<KeyId>,
    ) -> Vec<(KeyId, PublicKeyBytes)> {
        split_with_rng(key, threshold as u64, ids, rng)
            .unwrap()
            .into_iter()
            .map(|(id, secret)| (id, PublicKeyBytes::from(secret.public_key())))
            .collect()
    }

    /// A random validator key with its serialized public key and a valid share set for a
    /// committee with the given operator `ids`, split at the quorum threshold for that size.
    fn valid_committee(
        rng: &mut StdRng,
        ids: Vec<KeyId>,
    ) -> (PublicKeyBytes, Vec<(KeyId, PublicKeyBytes)>, usize) {
        let threshold = quorum_threshold(ids.len());
        let key = random_key(rng).unwrap();
        let shares = split_share_public_keys(rng, &key, threshold, ids);
        (PublicKeyBytes::from(key.public_key()), shares, threshold)
    }

    /// `valid_committee` for a committee of `committee_size` operators with ids
    /// `1..=committee_size`.
    fn valid_contiguous_committee(
        rng: &mut StdRng,
        committee_size: usize,
    ) -> (PublicKeyBytes, Vec<(KeyId, PublicKeyBytes)>, usize) {
        valid_committee(rng, key_ids(1..=committee_size as u64))
    }

    fn random_public_key_bytes(rng: &mut StdRng) -> PublicKeyBytes {
        PublicKeyBytes::from(random_key(rng).unwrap().public_key())
    }

    /// Compressed encoding of the G1 point at infinity: a well-formed encoding that
    /// `key_validate` must still reject.
    fn infinity_public_key_bytes() -> PublicKeyBytes {
        PublicKeyBytes::deserialize(&INFINITY_PUBLIC_KEY).unwrap()
    }

    // ==================== validate_share_public_keys: accepted share sets ====================

    #[test]
    fn test_validate_share_public_keys_accepts_split_shares_for_all_committee_sizes() {
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000001);

        for committee_size in COMMITTEE_SIZES {
            // Arrange
            let (validator_pubkey, shares, threshold) =
                valid_contiguous_committee(rng, committee_size);

            // Act
            let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

            // Assert
            assert_eq!(
                result,
                Ok(()),
                "committee of {committee_size} with threshold {threshold} should validate"
            );
        }
    }

    #[test]
    fn test_validate_share_public_keys_accepts_non_contiguous_ids() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000002);
        let (validator_pubkey, shares, threshold) = valid_committee(rng, key_ids(SPARSE_IDS));

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

        // Assert
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn test_validate_share_public_keys_accepts_go_ssv_committees() {
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED0000000C);

        for ids in GO_SSV_COMMITTEES {
            // Arrange
            let (validator_pubkey, shares, threshold) =
                valid_committee(rng, key_ids(ids.iter().copied()));

            // Act
            let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

            // Assert
            assert_eq!(result, Ok(()), "committee {ids:?} should validate");
        }
    }

    // ==================== validate_share_public_keys: rejected share sets ====================

    /// Replacing a share outside the base set must be reported against that share's id: the base
    /// still defines the correct polynomial, so only the tampered share fails to lie on it.
    #[test]
    fn test_validate_share_public_keys_rejects_tampered_remaining_share_naming_its_id() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000003);
        let (validator_pubkey, mut shares, threshold) = valid_contiguous_committee(rng, 4);
        let last = shares.len() - 1;
        let tampered_id = u64::from(shares[last].0.clone());
        shares[last].1 = random_public_key_bytes(rng);

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

        // Assert
        assert_eq!(
            result,
            Err(SharePublicKeyError::NotReconstructed { id: tampered_id })
        );
    }

    /// Replacing a share inside the base set is detected, but it cannot be attributed to the
    /// tampered id: the base shares are taken as ground truth for the polynomial, so the wrong
    /// polynomial makes every remaining share fail, and the error names the first of them (or,
    /// in the negligible case where the wrong polynomial happens to pass through the validator
    /// key, `DegreeBelowThreshold`). This mirrors go-ssv, which has the same limitation.
    #[test]
    fn test_validate_share_public_keys_rejects_tampered_base_share() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000004);
        let (validator_pubkey, mut shares, threshold) = valid_contiguous_committee(rng, 4);
        let tampered_id = u64::from(shares[0].0.clone());
        shares[0].1 = random_public_key_bytes(rng);

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

        // Assert
        assert!(result.is_err(), "tampered base share must be rejected");
        assert_ne!(
            result,
            Err(SharePublicKeyError::NotReconstructed { id: tampered_id }),
            "base shares are never the reported id, see doc comment"
        );
    }

    /// Splitting with a lower threshold than the one validated against yields a polynomial of
    /// degree `threshold - 2`, so the first `threshold - 1` shares already reconstruct the key.
    #[test]
    fn test_validate_share_public_keys_rejects_polynomial_degree_below_threshold() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000005);
        let committee_size = 4;
        let threshold = quorum_threshold(committee_size);
        let key = random_key(rng).unwrap();
        let shares =
            split_share_public_keys(rng, &key, threshold - 1, key_ids(1..=committee_size as u64));
        let validator_pubkey = PublicKeyBytes::from(key.public_key());

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

        // Assert
        assert_eq!(result, Err(SharePublicKeyError::DegreeBelowThreshold));
    }

    /// A consistent share set whose free coefficient is some other key: the base plus the first
    /// remaining share reconstructs that other key, not the validator key.
    #[test]
    fn test_validate_share_public_keys_rejects_shares_of_different_validator_key() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000006);
        let (_, shares, threshold) = valid_contiguous_committee(rng, 4);
        let other_validator_pubkey = random_public_key_bytes(rng);
        let first_remaining_id = u64::from(shares[threshold - 1].0.clone());

        // Act
        let result = validate_share_public_keys(&other_validator_pubkey, &shares, threshold);

        // Assert
        assert_eq!(
            result,
            Err(SharePublicKeyError::NotReconstructed {
                id: first_remaining_id
            })
        );
    }

    /// go-ssv "one share replaced": for every position, the share is swapped for the share of a
    /// different split at the same id. Base and remaining positions report different errors (see
    /// the two tampered-share tests above), so only rejection is asserted here.
    #[test]
    fn test_validate_share_public_keys_rejects_single_replaced_share_at_every_position() {
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED0000000D);

        for ids in GO_SSV_COMMITTEES {
            let (validator_pubkey, shares, threshold) =
                valid_committee(rng, key_ids(ids.iter().copied()));
            let (_, other_shares, _) = valid_committee(rng, key_ids(ids.iter().copied()));

            for position in 0..shares.len() {
                // Arrange
                let mut tampered = shares.clone();
                tampered[position].1 = other_shares[position].1;

                // Act
                let result = validate_share_public_keys(&validator_pubkey, &tampered, threshold);

                // Assert
                assert!(
                    result.is_err(),
                    "committee {ids:?}: replaced share at position {position} must be rejected"
                );
            }
        }
    }

    /// go-ssv "shares evaluated at wrong ids": the shares lie on one polynomial, but at ids
    /// offset by 1000 from the ones they are presented with. Interpolating at the presented ids
    /// evaluates a shifted polynomial whose free coefficient is not the validator key, so the
    /// first share past the base is reported.
    #[test]
    fn test_validate_share_public_keys_rejects_shares_evaluated_at_wrong_ids() {
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED0000000E);

        for ids in GO_SSV_COMMITTEES {
            // Arrange
            let threshold = quorum_threshold(ids.len());
            let key = random_key(rng).unwrap();
            let shifted_shares = split_share_public_keys(
                rng,
                &key,
                threshold,
                key_ids(ids.iter().map(|id| id + 1000)),
            );
            let shares = key_ids(ids.iter().copied())
                .into_iter()
                .zip(shifted_shares)
                .map(|(id, (_, pubkey))| (id, pubkey))
                .collect::<Vec<_>>();
            let validator_pubkey = PublicKeyBytes::from(key.public_key());

            // Act
            let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

            // Assert
            assert_eq!(
                result,
                Err(SharePublicKeyError::NotReconstructed {
                    id: ids[threshold - 1]
                }),
                "committee {ids:?}: shares at ids + 1000 must be rejected"
            );
        }
    }

    /// go-ssv "polynomial degree too low", for every degree from 0 to `quorum - 2`. Degree 0
    /// (every share equal to the validator key) is built by hand because `split_with_rng`
    /// rejects thresholds below 2; the remaining degrees are split at `threshold = degree + 1`.
    #[test]
    fn test_validate_share_public_keys_rejects_every_polynomial_degree_below_threshold() {
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED0000000F);

        for ids in GO_SSV_COMMITTEES {
            let threshold = quorum_threshold(ids.len());
            let key = random_key(rng).unwrap();
            let validator_pubkey = PublicKeyBytes::from(key.public_key());

            for low_threshold in 1..threshold {
                // Arrange
                let shares = if low_threshold == 1 {
                    key_ids(ids.iter().copied())
                        .into_iter()
                        .map(|id| (id, validator_pubkey))
                        .collect()
                } else {
                    split_share_public_keys(rng, &key, low_threshold, key_ids(ids.iter().copied()))
                };

                // Act
                let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

                // Assert
                assert_eq!(
                    result,
                    Err(SharePublicKeyError::DegreeBelowThreshold),
                    "committee {ids:?}: polynomial degree {} must be rejected",
                    low_threshold - 1
                );
            }
        }
    }

    /// go-ssv "polynomial degree too high": shares split at `threshold + 1` lie on a polynomial
    /// of degree `threshold`, so `threshold` points (the base plus the first remaining share) do
    /// not determine it and interpolation misses the validator key at that share.
    #[test]
    fn test_validate_share_public_keys_rejects_polynomial_degree_above_threshold() {
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000010);

        for ids in GO_SSV_COMMITTEES {
            // Arrange
            let threshold = quorum_threshold(ids.len());
            let key = random_key(rng).unwrap();
            let shares =
                split_share_public_keys(rng, &key, threshold + 1, key_ids(ids.iter().copied()));
            let validator_pubkey = PublicKeyBytes::from(key.public_key());

            // Act
            let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

            // Assert
            assert_eq!(
                result,
                Err(SharePublicKeyError::NotReconstructed {
                    id: ids[threshold - 1]
                }),
                "committee {ids:?}: polynomial degree {threshold} must be rejected"
            );
        }
    }

    /// go-ssv "shares out of order": the first and last share public keys are swapped while the
    /// ids stay in place, so two shares are presented at the wrong ids.
    #[test]
    fn test_validate_share_public_keys_rejects_swapped_first_and_last_share_public_keys() {
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000011);

        for ids in GO_SSV_COMMITTEES {
            // Arrange
            let (validator_pubkey, mut shares, threshold) =
                valid_committee(rng, key_ids(ids.iter().copied()));
            let last = shares.len() - 1;
            let first_pubkey = shares[0].1;
            shares[0].1 = shares[last].1;
            shares[last].1 = first_pubkey;

            // Act
            let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

            // Assert
            assert!(
                result.is_err(),
                "committee {ids:?}: swapped first and last share public keys must be rejected"
            );
        }
    }

    // ==================== validate_share_public_keys: input validation ====================

    #[test]
    fn test_validate_share_public_keys_rejects_threshold_below_two() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000007);
        let (validator_pubkey, shares, _) = valid_contiguous_committee(rng, 4);

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares, 1);

        // Assert
        assert_eq!(
            result,
            Err(SharePublicKeyError::InvalidThreshold {
                threshold: 1,
                shares: 4
            })
        );
    }

    #[test]
    fn test_validate_share_public_keys_rejects_threshold_above_share_count() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000008);
        let (validator_pubkey, shares, _) = valid_contiguous_committee(rng, 4);

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares, 5);

        // Assert
        assert_eq!(
            result,
            Err(SharePublicKeyError::InvalidThreshold {
                threshold: 5,
                shares: 4
            })
        );
    }

    #[test]
    fn test_validate_share_public_keys_rejects_all_zero_share_public_key() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED00000009);
        let (validator_pubkey, mut shares, threshold) = valid_contiguous_committee(rng, 4);
        let invalid_id = u64::from(shares[2].0.clone());
        shares[2].1 = PublicKeyBytes::empty();

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

        // Assert
        assert_eq!(
            result,
            Err(SharePublicKeyError::InvalidSharePublicKey { id: invalid_id })
        );
    }

    #[test]
    fn test_validate_share_public_keys_rejects_infinity_share_public_key() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED0000000A);
        let (validator_pubkey, mut shares, threshold) = valid_contiguous_committee(rng, 4);
        let invalid_id = u64::from(shares[0].0.clone());
        shares[0].1 = infinity_public_key_bytes();

        // Act
        let result = validate_share_public_keys(&validator_pubkey, &shares, threshold);

        // Assert
        assert_eq!(
            result,
            Err(SharePublicKeyError::InvalidSharePublicKey { id: invalid_id })
        );
    }

    #[test]
    fn test_validate_share_public_keys_rejects_invalid_validator_public_key() {
        // Arrange
        let rng = &mut StdRng::seed_from_u64(0x5A4E5EED0000000B);
        let (_, shares, threshold) = valid_contiguous_committee(rng, 4);

        // Act
        let all_zero = validate_share_public_keys(&PublicKeyBytes::empty(), &shares, threshold);
        let infinity = validate_share_public_keys(&infinity_public_key_bytes(), &shares, threshold);

        // Assert
        assert_eq!(
            all_zero,
            Err(SharePublicKeyError::InvalidValidatorPublicKey)
        );
        assert_eq!(
            infinity,
            Err(SharePublicKeyError::InvalidValidatorPublicKey)
        );
    }
}
