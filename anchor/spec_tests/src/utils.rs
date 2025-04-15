use std::collections::HashMap;

use hex::FromHex;
use openssl::{pkey::Private, rsa::Rsa};
use ssv_types::OperatorId;
use types::{PublicKeyBytes, SecretKey};

use crate::constants::*;

// Reimplementation of required testing infrastruture
// https://github.com/ssvlabs/ssv-spec/blob/main/types/testingutils/keys.go#L76

// A SSV Operator that is responsible for signing
pub struct TestingSigner {
    private_key: Rsa<Private>,
    operator_id: OperatorId,
}

impl TestingSigner {
    pub fn new_testing_signer(keyset: &TestKeySet, id: u64) -> TestingSigner {
        let id = OperatorId::from(id);
        TestingSigner {
            private_key: keyset.operator_keys.get(&id).unwrap().clone(),
            operator_id: id,
        }
    }
}

pub struct TestKeySet {
    pub secret_key: SecretKey,
    pub public_key: PublicKeyBytes,
    pub share_count: u64,
    pub threshold: u64,
    pub partial_threshold: u64,
    pub shares: HashMap<OperatorId, SecretKey>,
    pub operator_keys: HashMap<OperatorId, Rsa<Private>>,
    // TODO!() operators: HashMap<OperatorId, Operator>,
}

impl TestKeySet {
    #[rustfmt::skip]
    pub fn four_share_set() -> TestKeySet {
        TestKeySet {
            secret_key: VALIDATOR_SECRET_KEY.clone(),
            public_key: *TESTING_VALIDATOR_PUBKEY,
            share_count: 4,
            threshold: 3,
            partial_threshold: 2,
            shares: HashMap::from([
                (OperatorId::from(1),secret_key_from_hex("5f4711a796c1116b5118ec35279fb64d551d9b38813d2939954dd2df5160d3d9")),
                (OperatorId::from(2),secret_key_from_hex("48e4c0a38e90f9352d1d09489446443ebd17b1904f4f0002fe894c2c3f62457a")),
                (OperatorId::from(3),secret_key_from_hex("65dc7c179f68347cf12f86e1c51e54e8aeeed579d4c715082bb8a0382c1a8153")),
                (OperatorId::from(4),secret_key_from_hex("42409cb09fa945fa6a168cf8b0861045d6e562f211a70c4a1cdbcf0417898763")),
            ]),
            operator_keys: HashMap::from([
                (OperatorId::from(1),rsa_secret_from_hex(FOUR_OPERATOR_ONE_PRIVATE)),
                (OperatorId::from(2),rsa_secret_from_hex(FOUR_OPERATOR_TWO_PRIVATE)),
                (OperatorId::from(3),rsa_secret_from_hex(FOUR_OPERATOR_THREE_PRIVATE)),
                (OperatorId::from(4),rsa_secret_from_hex(FOUR_OPERATOR_FOUR_PRIVATE)),
            ]),
        }
    }

    fn seven_share_set() -> TestKeySet {
        todo!()
    }

    fn ten_share_set() -> TestKeySet {
        todo!()
    }

    fn thirteen_share_set() -> TestKeySet {
        todo!()
    }
}

pub fn secret_key_from_hex(hex: &str) -> SecretKey {
    let bytes = <[u8; 32]>::from_hex(hex).expect("Invalid hex string");
    SecretKey::deserialize(&bytes).expect("Failed to create secret key")
}

pub fn rsa_secret_from_hex(key: &str) -> Rsa<Private> {
    let pem_bytes = hex::decode(key).expect("Valid key");
    Rsa::private_key_from_der(&pem_bytes).expect("Valid key bytes")
}
