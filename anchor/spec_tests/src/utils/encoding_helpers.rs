use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

/// SSZ roundtrip: decode → encode → compare bytes.
pub fn check_roundtrip<T: Decode + Encode>(data: &[u8]) -> Result<(), String> {
    let decoded = T::from_ssz_bytes(data).map_err(|e| format!("SSZ decode failed: {e:?}"))?;
    let encoded = decoded.as_ssz_bytes();
    if encoded != data {
        return Err(format!(
            "SSZ roundtrip mismatch: encoded {} bytes, expected {} bytes",
            encoded.len(),
            data.len()
        ));
    }
    Ok(())
}

/// SSZ roundtrip + hash tree root verification.
pub fn check_roundtrip_with_root<T: Decode + Encode + TreeHash>(
    data: &[u8],
    expected_root: Hash256,
) -> Result<(), String> {
    let decoded = T::from_ssz_bytes(data).map_err(|e| format!("SSZ decode failed: {e:?}"))?;
    let encoded = decoded.as_ssz_bytes();
    if encoded != data {
        return Err(format!(
            "SSZ roundtrip mismatch: encoded {} bytes, expected {} bytes",
            encoded.len(),
            data.len()
        ));
    }
    let root = decoded.tree_hash_root();
    if root != expected_root {
        return Err(format!(
            "Hash tree root mismatch: got {root:?}, expected {expected_root:?}",
        ));
    }
    Ok(())
}
