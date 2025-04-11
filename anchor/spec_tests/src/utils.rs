use std::collections::HashMap;

use hex::FromHex;
use openssl::{pkey::Private, rsa::Rsa};
use ssv_types::OperatorId;
use types::{PublicKeyBytes, SecretKey};

use crate::constants::*;

// Reimplementation of required testing constants
// https://github.com/ssvlabs/ssv-spec/blob/main/types/testingutils/keys.go#L76

pub struct TestKeySet {
    secret_key: SecretKey,
    public_key: PublicKeyBytes,
    share_count: u64,
    threshold: u64,
    partial_threshold: u64,
    shares: HashMap<OperatorId, SecretKey>,
    operator_keys: HashMap<OperatorId, Rsa<Private>>,
    // TODO!() operators: HashMap<OperatorId, Operator>,
}

impl TestKeySet {
    pub fn four_share_set() -> TestKeySet {
        TestKeySet {
            secret_key: VALIDATOR_SECRET_KEY.clone(),
            public_key: *TESTING_VALIDATOR_PUBKEY,
            share_count: 4,
            threshold: 3,
            partial_threshold: 2,
            shares: HashMap::from([
                (
                    OperatorId::from(1),
                    secret_key_from_hex(
                        "5f4711a796c1116b5118ec35279fb64d551d9b38813d2939954dd2df5160d3d9",
                    ),
                ),
                (
                    OperatorId::from(2),
                    secret_key_from_hex(
                        "48e4c0a38e90f9352d1d09489446443ebd17b1904f4f0002fe894c2c3f62457a",
                    ),
                ),
                (
                    OperatorId::from(3),
                    secret_key_from_hex(
                        "65dc7c179f68347cf12f86e1c51e54e8aeeed579d4c715082bb8a0382c1a8153",
                    ),
                ),
                (
                    OperatorId::from(4),
                    secret_key_from_hex(
                        "42409cb09fa945fa6a168cf8b0861045d6e562f211a70c4a1cdbcf0417898763",
                    ),
                ),
            ]),
            operator_keys: HashMap::from([
                (
                    OperatorId::from(1),
                    rsa_secret_from_hex(FOUR_OPERATOR_ONE_PUBLIC),
                ),
                (
                    OperatorId::from(2),
                    rsa_secret_from_hex(FOUR_OPERATOR_TWO_PUBLIC),
                ),
                (
                    OperatorId::from(3),
                    rsa_secret_from_hex(FOUR_OPERATOR_THREE_PUBLIC),
                ),
                (
                    OperatorId::from(4),
                    rsa_secret_from_hex(FOUR_OPERATOR_FOUR_PUBLIC),
                ),
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

pub fn rsa_secret_from_hex(hex: &str) -> Rsa<Private> {
    todo!()
}
