use bls::SecretKey;
use rand::prelude::*;

#[cfg(feature = "blst")]
pub mod blst;
#[cfg(all(not(feature = "blsful"), feature = "blst"))]
pub use self::blst::*;
#[cfg(feature = "blsful")]
pub mod blsful;
#[cfg(feature = "blsful")]
pub use self::blsful::*;

#[derive(Debug, Clone, Copy)]
pub enum Error {
    InternalError,
    InvalidThreshold,
    LessThanTwoSignatures,
    NotOneIdPerSignature,
    ZeroId,
    ZeroKey,
    RepeatedId,
    InvalidSignature,
}

pub fn split(
    key: &SecretKey,
    threshold: u64,
    ids: impl IntoIterator<Item = KeyId>,
) -> Result<Vec<(KeyId, SecretKey)>, Error> {
    split_with_rng(key, threshold, ids, &mut thread_rng())
}

#[cfg(any(feature = "blst", test))]
pub(crate) fn random_key(rng: &mut (impl CryptoRng + Rng)) -> Result<SecretKey, Error> {
    let ikm = zeroize::Zeroizing::new(rng.r#gen::<[u8; 32]>());
    let sk =
        ::blst::min_pk::SecretKey::key_gen(ikm.as_ref(), &[]).map_err(|_| Error::InternalError)?;
    // By passing a reference here, we drop "sk", zeroizing it.
    Ok(SecretKey::from_point(&sk))
}

#[cfg(test)]
mod tests {
    use std::{hint::black_box, mem, time::Instant};

    use ::blst::{blst_scalar, blst_scalar_from_le_bytes};
    use bls::{Hash256, Signature};

    use super::*;

    #[test]
    fn test_basic_often() {
        let mut rng = &mut StdRng::seed_from_u64(0x12345EED00000000);
        for _ in 0..1000 {
            test_basic(&mut rng);
        }
    }

    fn test_basic(rng: &mut (impl CryptoRng + Rng)) {
        let total = rng.gen_range(2..=13);
        let threshold = rng.gen_range(2..=total);

        let master = random_key(rng).unwrap();
        let pk = master.public_key();

        let mut keys = split_with_rng(
            &master,
            threshold as u64,
            (1..=total).map(|x| KeyId::try_from(x as u64).unwrap()),
            rng,
        )
        .unwrap();

        // shuffle to sign with varying key indices
        keys.shuffle(rng);

        let (ids, keys): (Vec<_>, Vec<_>) = keys.into_iter().unzip();

        assert_eq!(keys.len(), total);

        let mut data = [0u8; 32];
        rng.fill(&mut data);

        let signers = rng.gen_range(2..=total);

        let signatures = keys
            .into_iter()
            .take(signers)
            .map(|key| key.sign(Hash256::from(data)))
            .collect::<Vec<_>>();

        let combined = combine_signatures(&signatures, &ids[..signers]).unwrap();

        let result = combined.verify(&pk, data.into());
        if signers >= threshold {
            assert!(result);
        } else {
            assert!(!result);
        }
    }

    #[test]
    fn bench_basic() {
        println!("1000 iterations each");
        (1..=4).for_each(do_bench_basic)
    }

    fn do_bench_basic(f: usize) {
        let rng = &mut StdRng::seed_from_u64(0x12345EED00000000);
        let total = 3 * f + 1;
        let threshold = 2 * f + 1;

        let master = random_key(rng).unwrap();

        let mut keys = split_with_rng(
            &master,
            threshold as u64,
            (1..=total).map(|x| KeyId::try_from(x as u64).unwrap()),
            rng,
        )
        .unwrap();

        // shuffle to sign with varying key indices
        keys.shuffle(rng);

        let (ids, keys): (Vec<_>, Vec<_>) = keys.into_iter().unzip();

        assert_eq!(keys.len(), total);

        let mut data = [0u8; 32];
        rng.fill(&mut data);

        let signatures = keys
            .into_iter()
            .take(threshold)
            .map(|key| key.sign(Hash256::from(data)))
            .collect::<Vec<_>>();

        let timing = Instant::now();
        for _ in 0..1_000 {
            black_box(combine_signatures(&signatures, &ids[..threshold]).unwrap());
        }
        println!(
            "took {} ms for threshold = {threshold}",
            timing.elapsed().as_millis()
        );
    }

    #[test]
    fn test_invalid_threshold() {
        let rng = &mut StdRng::seed_from_u64(0x12345EED00000123);
        let key = random_key(rng).unwrap();
        assert!(matches!(
            split_with_rng(
                &key,
                1,
                (1..=10).map(|x| KeyId::try_from(x as u64).unwrap()),
                rng
            ),
            Err(Error::InvalidThreshold)
        ));
        assert!(matches!(
            split_with_rng(
                &key,
                0,
                (144..=166).map(|x| KeyId::try_from(x as u64).unwrap()),
                rng
            ),
            Err(Error::InvalidThreshold)
        ));
    }

    #[test]
    fn test_less_than_two_sigs() {
        let signature = [Signature::empty()];
        let key_id = [KeyId::try_from(97).unwrap()];
        assert!(matches!(
            combine_signatures(&signature, &key_id),
            Err(Error::LessThanTwoSignatures)
        ));
    }

    #[test]
    fn test_not_one_id_per_signature() {
        let signatures = [Signature::empty(), Signature::infinity().unwrap()];
        let key_ids = [KeyId::try_from(4).unwrap()];
        assert!(matches!(
            combine_signatures(&signatures, &key_ids),
            Err(Error::NotOneIdPerSignature)
        ));
        let signatures = [Signature::infinity().unwrap(), Signature::empty()];
        let key_ids = [
            KeyId::try_from(2).unwrap(),
            KeyId::try_from(1).unwrap(),
            KeyId::try_from(4).unwrap(),
        ];
        assert!(matches!(
            combine_signatures(&signatures, &key_ids),
            Err(Error::NotOneIdPerSignature)
        ));
    }

    #[test]
    fn test_zero_id() {
        assert!(matches!(KeyId::try_from(0), Err(Error::ZeroId)));
    }

    #[test]
    fn test_zero_key() {
        let rng = &mut StdRng::seed_from_u64(0x12345EED55500000);
        // it's not easy to get a zero key in the first place...
        let key = SecretKey::from_point(unsafe {
            let mut scalar = blst_scalar::default();
            blst_scalar_from_le_bytes(&mut scalar, &0u8, 1);
            &mem::transmute::<blst_scalar, ::blst::min_pk::SecretKey>(scalar)
        });
        assert!(matches!(
            split_with_rng(
                &key,
                3,
                (1..=10).map(|x| KeyId::try_from(x as u64).unwrap()),
                rng
            ),
            Err(Error::ZeroKey)
        ));
    }

    #[test]
    fn test_repeated_id() {
        let rng = &mut StdRng::seed_from_u64(0xF2345EED0000000);
        let master = random_key(rng).unwrap();
        let keys = split_with_rng(
            &master,
            3,
            (11..=15).map(|x| KeyId::try_from(x as u64).unwrap()),
            rng,
        )
        .unwrap();

        let (ids, keys): (Vec<_>, Vec<_>) = keys.into_iter().unzip();

        let mut data = [0u8; 32];
        rng.fill(&mut data);

        let signers = [0, 1, 1];

        let signatures = signers
            .iter()
            .map(|signer| keys[*signer].sign(Hash256::from(data)))
            .collect::<Vec<_>>();
        let ids = signers
            .into_iter()
            .map(|signer| ids[signer].clone())
            .collect::<Vec<_>>();

        assert!(matches!(
            combine_signatures(&signatures, &ids),
            Err(Error::RepeatedId)
        ));
    }

    #[test]
    fn test_invalid_signature() {
        let signature = [Signature::empty(), Signature::empty()];
        let key_id = [KeyId::try_from(99).unwrap(), KeyId::try_from(98).unwrap()];
        assert!(matches!(
            combine_signatures(&signature, &key_id),
            Err(Error::InvalidSignature)
        ));
    }
}
