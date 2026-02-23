use serde::Deserialize;
use types::Hash256;

use crate::{
    SpecTest,
    utils::{
        check_roundtrip_with_root,
        deserializers::{deserialize_base64, deserialize_bytes_to_hash256},
    },
};

/// Mirrors Go's `EncodingTest` for `PartialSignatureMessages`.
///
/// Validates SSZ encode/decode roundtrip and hash tree root.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PartialSigMessageEncodingTest {
    #[serde(deserialize_with = "deserialize_base64")]
    data: Vec<u8>,
    #[serde(deserialize_with = "deserialize_bytes_to_hash256")]
    expected_root: Hash256,
}

impl SpecTest for PartialSigMessageEncodingTest {
    fn run(&self) -> Result<(), String> {
        check_roundtrip_with_root::<ssv_types::partial_sig::PartialSignatureMessages>(
            &self.data,
            self.expected_root,
        )
    }
}
