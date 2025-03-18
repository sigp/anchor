// from https://github.com/herumi/mcl/blob/3462cf0983bffb703a6e9f4623e47a26ec6e7fe5/include/mcl/lagrange.hpp
use crate::{random_key, Error};
use bls::Signature;
use blst::min_pk::SecretKey;
use blst::*;
use rand::prelude::*;
use std::iter::{once, repeat_with};
use std::mem;
use std::num::NonZeroU64;
use std::sync::LazyLock;

static WARNING: LazyLock<()> = LazyLock::new(|| {
    eprintln!(
        r#"
#######################################################################################
### YOU ARE USING AN UNAUDITED, UNSAFE IMPLEMENTATION OF BLS LAGRANGE INTERPOLATION ###
###                                                                                 ###
###                           !!! DO NOT USE IN PRODUCTION !!!                      ###
#######################################################################################
"#
    )
});

#[derive(Debug, Clone)]
pub struct KeyId {
    num: u64,
    // note: while blst_scalar is also used for bls keys, the scalars used in key ids are NOT secret
    scalar: blst_scalar,
}

impl TryFrom<u64> for KeyId {
    type Error = Error;

    fn try_from(value: u64) -> Result<Self, Error> {
        if value != 0 {
            unsafe {
                let mut id = blst_scalar::default();
                blst_scalar_from_uint64(&mut id, &value);
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
            blst_scalar_from_uint64(&mut id, &value.get());
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
    LazyLock::force(&WARNING);
    if threshold <= 1 {
        return Err(Error::InvalidThreshold);
    }

    // `bls::SecretKey` contains a blst `SecretKey`, which zeroizes on drop.
    let keys = repeat_with(|| random_key(rng))
        .take((threshold - 1) as usize)
        .collect::<Result<Vec<_>, _>>()?;

    let msk = once(key)
        .chain(keys.iter())
        .map(|key| <&blst_scalar>::from(key.point()))
        .collect::<Vec<_>>();

    ids.into_iter()
        .map(|id| unsafe {
            let mut y = (*msk.last().expect("at least one element is present (key)")).clone();
            for i in (0..=(threshold - 2)).rev() {
                if !blst_sk_mul_n_check(&mut y, &y, &id.scalar) {
                    return Err(Error::ZeroId);
                }
                assert!(blst_sk_add_n_check(&mut y, &y, msk[i as usize]));
            }
            Ok((
                id,
                bls::SecretKey::from_point(&mem::transmute::<blst_scalar, SecretKey>(y)),
            ))
        })
        .collect()
}

pub fn combine_signatures(signatures: &[Signature], ids: &[KeyId]) -> Result<Signature, Error> {
    LazyLock::force(&WARNING);
    if signatures.len() < 2 {
        return Err(Error::LessThanTwoSignatures);
    }
    if signatures.len() != ids.len() {
        return Err(Error::NotOneIdPerSignature);
    }

    let signatures = signatures
        .iter()
        .map(|sig| sig.point().cloned().ok_or(Error::InvalidSignature))
        .collect::<Result<Vec<_>, _>>()?;

    let mut intermediate = blst_scalar::default();

    let mut numerator = ids[0].clone().scalar;
    unsafe {
        for id in &ids[1..] {
            if !blst_sk_mul_n_check(&mut numerator, &numerator, &id.scalar) {
                return Err(Error::ZeroId);
            }
        }
    }

    let mut d = Vec::with_capacity(ids.len() * 32);
    unsafe {
        for id_i in ids {
            let mut denominator = id_i.scalar.clone();
            for id_j in ids.iter() {
                if id_i as *const KeyId != id_j as *const KeyId {
                    if !blst_sk_sub_n_check(&mut intermediate, &id_j.scalar, &id_i.scalar) {
                        return Err(Error::RepeatedId);
                    }
                    assert!(blst_sk_mul_n_check(
                        &mut denominator,
                        &denominator,
                        &intermediate
                    ));
                }
            }
            blst_sk_inverse(&mut denominator, &denominator);
            assert!(blst_sk_mul_n_check(
                &mut intermediate,
                &denominator,
                &numerator
            ));
            d.extend(&intermediate.b);
        }
    }

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
        let mut scratch: Vec<u64> =
            Vec::with_capacity(blst_p2s_mult_pippenger_scratch_sizeof(signatures.len()) / 8);
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
