use serde::Deserialize;
use ssz::{Decode, Encode};

use crate::{SpecTest, utils::deserializers::deserialize_base64};

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
        // Decode SignedSSVMessage from SSZ bytes
        let decoded = ssv_types::message::SignedSSVMessage::from_ssz_bytes(&self.data)
            .map_err(|e| format!("SSZ decode failed: {e:?}"))?;

        // Encode back and verify roundtrip
        let encoded = decoded.as_ssz_bytes();
        if encoded != self.data {
            return Err(format!(
                "SSZ roundtrip mismatch: encoded {} bytes, expected {} bytes",
                encoded.len(),
                self.data.len()
            ));
        }

        Ok(())
    }
}
