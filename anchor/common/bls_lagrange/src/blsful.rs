use std::num::NonZeroU64;

use blstrs_plus::{G2Projective, Scalar};
use rand::{CryptoRng, Rng};
use vsss_rs::{
    IdentifierPrimeField, ParticipantIdGeneratorType, ReadableShareSet, ValueGroup,
    elliptic_curve::Field, shamir,
};
use zeroize::Zeroizing;

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyId {
    num: u64,
    identifier: IdentifierPrimeField<Scalar>,
}

impl TryFrom<u64> for KeyId {
    type Error = Error;

    fn try_from(value: u64) -> Result<Self, Error> {
        if value != 0 {
            Ok(KeyId {
                num: value,
                identifier: IdentifierPrimeField(Scalar::from(value)),
            })
        } else {
            Err(Error::ZeroId)
        }
    }
}
impl From<NonZeroU64> for KeyId {
    fn from(value: NonZeroU64) -> Self {
        KeyId {
            num: value.get(),
            identifier: IdentifierPrimeField(Scalar::from(value.get())),
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
    let result = Scalar::from_be_bytes(
        key.serialize()
            .as_bytes()
            .try_into()
            .map_err(|_| Error::InternalError)?,
    );
    let scalar = result.into_option().ok_or(Error::InternalError)?;
    if bool::from(scalar.is_zero()) {
        return Err(Error::ZeroKey);
    }
    let key = Zeroizing::new(IdentifierPrimeField(scalar));

    let ids = ids.into_iter().map(|k| k.identifier).collect::<Vec<_>>();

    let result = Zeroizing::new(shamir::split_secret_with_participant_generator(
        threshold as usize,
        ids.len(),
        &*key,
        rng,
        &[ParticipantIdGeneratorType::List { list: &ids }],
    )?);

    result
        .iter()
        .map(|(identifier, share)| {
            bls::SecretKey::deserialize(&share.0.to_be_bytes())
                .map_err(|_| Error::InternalError)
                .map(move |sk| {
                    let bytes = identifier.0.to_be_bytes();
                    debug_assert_eq!(bytes[..24], [0; 24]);
                    (
                        KeyId {
                            num: u64::from_be_bytes((&bytes[24..]).try_into().unwrap()),
                            identifier: *identifier,
                        },
                        sk,
                    )
                })
        })
        .collect()
}

pub fn combine_signatures(
    signatures: &[bls::Signature],
    ids: &[KeyId],
) -> Result<bls::Signature, Error> {
    if signatures.len() < 2 {
        return Err(Error::LessThanTwoSignatures);
    }
    if signatures.len() != ids.len() {
        return Err(Error::NotOneIdPerSignature);
    }

    let share_set = signatures
        .iter()
        .zip(ids)
        .map(|(sig, id)| {
            let Some(bytes) = sig.serialize_uncompressed() else {
                return Err(Error::InvalidSignature);
            };
            let g2 = G2Projective::from_uncompressed(&bytes);
            if g2.is_some().into() {
                Ok((id.identifier, ValueGroup(g2.unwrap())))
            } else {
                Err(Error::InternalError)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    let result = share_set.combine()?;
    bls::Signature::deserialize_uncompressed(&result.0.to_uncompressed())
        .map_err(|_| Error::InternalError)
}

impl From<vsss_rs::Error> for Error {
    fn from(value: vsss_rs::Error) -> Self {
        match value {
            vsss_rs::Error::SharingMinThreshold => Error::InvalidThreshold,
            vsss_rs::Error::SharingLimitLessThanThreshold => Error::InvalidThreshold,
            vsss_rs::Error::SharingInvalidIdentifier => Error::ZeroId,
            vsss_rs::Error::SharingDuplicateIdentifier => Error::RepeatedId,
            vsss_rs::Error::InvalidSecret => Error::ZeroKey,
            _ => Error::InternalError,
        }
    }
}
