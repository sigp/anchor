// from https://github.com/herumi/mcl/blob/3462cf0983bffb703a6e9f4623e47a26ec6e7fe5/include/mcl/lagrange.hpp
use std::{
    iter::{once, repeat_with},
    mem,
    num::NonZeroU64,
};

use bls::Signature;
use blst::{min_pk::SecretKey, *};
use rand::prelude::*;

use crate::{random_key, Error};

#[derive(Debug, Clone)]
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

    let mut intermediate = blst_scalar::default();

    // "numerator" is the product of all key ids
    let mut numerator = ids[0].clone().scalar;
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
                if id_i as *const KeyId != id_j as *const KeyId {
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

    // `mult` multiplies the signatures and scalars pairwise, then sums them up.
    Ok(Signature::from_point(mult(&signatures, &d), false))
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
}
