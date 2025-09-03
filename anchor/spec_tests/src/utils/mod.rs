use std::str::FromStr;
use std::sync::LazyLock;
use types::PublicKeyBytes;

pub mod deserializers;

pub static TESTING_VALIDATOR_PUBKEY: LazyLock<PublicKeyBytes> = LazyLock::new(|| {
    PublicKeyBytes::from_str("0x8e80066551a81b318258709edaf7dd1f63cd686a0e4db8b29bbb7acfe65608677af5a527d9448ee47835485e02b50bc0").expect("Failed to create public key")
});
