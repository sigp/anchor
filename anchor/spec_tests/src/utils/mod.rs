use std::{str::FromStr, sync::LazyLock};

use bls::PublicKeyBytes;

pub mod deserializers;
pub mod dtos;
pub mod encoding_helpers;
pub mod error_codes;

pub use encoding_helpers::{
    can_decode_block, check_roundtrip, check_roundtrip_with_root, decode_base64,
    is_bls_validation_error,
};

/// Matches Go's `TestingValidatorPubKey`:
/// https://github.com/ssvlabs/ssv-spec/blob/45153e4e4b8c61b929f701b7af52e3f725668421/types/testingutils/keys.go#L16-L22
pub static TESTING_VALIDATOR_PUBKEY: LazyLock<PublicKeyBytes> = LazyLock::new(|| {
    PublicKeyBytes::from_str(
        "0x8e80066551a81b318258709edaf7dd1f63cd686a0e4db8b29bbb7acfe65608677af5a527d9448ee47835485e02b50bc0",
    )
    .expect("Failed to create public key")
});
