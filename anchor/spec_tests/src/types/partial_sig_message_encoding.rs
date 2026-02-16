use serde::Deserialize;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    SpecTest,
    utils::deserializers::{deserialize_base64, deserialize_bytes_to_hash256},
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
        // Decode PartialSignatureMessages from SSZ bytes
        let decoded = ssv_types::partial_sig::PartialSignatureMessages::from_ssz_bytes(&self.data)
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

        // Verify hash tree root
        let root = decoded.tree_hash_root();
        if root != self.expected_root {
            return Err(format!(
                "Hash tree root mismatch: got {root:?}, expected {:?}",
                self.expected_root
            ));
        }

        Ok(())
    }
}
