use serde::Deserialize;

use crate::{
    SpecTest,
    utils::{check_roundtrip, deserializers::deserialize_base64},
};

/// Mirrors Go's `EncodingTest` for `SignedSSVMessage`.
///
/// Validates SSZ encode/decode roundtrip (no tree hash root check).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SignedSSVMessageEncodingTest {
    #[serde(deserialize_with = "deserialize_base64")]
    data: Vec<u8>,
}

impl SpecTest for SignedSSVMessageEncodingTest {
    fn run(&self) -> Result<(), String> {
        check_roundtrip::<ssv_types::message::SignedSSVMessage>(&self.data)
    }
}
