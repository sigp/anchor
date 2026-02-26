use base64::{Engine, engine::general_purpose::STANDARD};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

/// Decode a base64 string into bytes.
pub fn decode_base64(s: &str) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(s)
        .map_err(|e| format!("base64 decode error: {e}"))
}

/// SSZ roundtrip: decode → encode → compare bytes. Returns decoded value.
pub fn check_roundtrip<T: Decode + Encode>(data: &[u8]) -> Result<T, String> {
    let decoded = T::from_ssz_bytes(data).map_err(|e| format!("SSZ decode failed: {e:?}"))?;
    let encoded = decoded.as_ssz_bytes();
    if encoded != data {
        return Err(format!(
            "SSZ roundtrip mismatch: encoded {} bytes, expected {} bytes",
            encoded.len(),
            data.len()
        ));
    }
    Ok(decoded)
}

/// SSZ roundtrip + hash tree root verification.
pub fn check_roundtrip_with_root<T: Decode + Encode + TreeHash>(
    data: &[u8],
    expected_root: Hash256,
) -> Result<(), String> {
    let decoded = check_roundtrip::<T>(data)?;
    let root = decoded.tree_hash_root();
    if root != expected_root {
        return Err(format!(
            "Hash tree root mismatch: got {root:?}, expected {expected_root:?}",
        ));
    }
    Ok(())
}
