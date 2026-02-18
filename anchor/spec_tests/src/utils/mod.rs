use std::{str::FromStr, sync::LazyLock};

use bls::PublicKeyBytes;

pub mod deserializers;
pub mod encoding_helpers;

pub use encoding_helpers::{check_roundtrip, check_roundtrip_with_root};

pub static TESTING_VALIDATOR_PUBKEY: LazyLock<PublicKeyBytes> = LazyLock::new(|| {
    PublicKeyBytes::from_str(
        "0x8e80066551a81b318258709edaf7dd1f63cd686a0e4db8b29bbb7acfe65608677af5a527d9448ee47835485e02b50bc0",
    )
    .expect("Failed to create public key")
});
