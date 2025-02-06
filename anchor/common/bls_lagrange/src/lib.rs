//! THIS CRATE IS NOT READY FOR PRODUCTION USE! DO *NOT* USE IN PRODUCTION CODE!

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
    RepeatedId,
    InvalidSignature,
}

pub fn split(
    key: SecretKey,
    threshold: u64,
    ids: impl IntoIterator<Item = KeyId>,
) -> Result<Vec<(KeyId, SecretKey)>, Error> {
    split_with_rng(key, threshold, ids, &mut thread_rng())
}

#[cfg(any(feature = "blst", test))]
pub(crate) fn random_key(rng: &mut (impl CryptoRng + Rng)) -> Result<SecretKey, Error> {
    let ikm: [u8; 32] = rng.gen();
    let sk = ::blst::min_pk::SecretKey::key_gen(&ikm, &[]).map_err(|_| Error::InternalError)?;
    Ok(SecretKey::from_point(sk))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bls::Hash256;
    use std::hint::black_box;
    use std::time::Instant;

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
            master,
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
            master,
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
}
