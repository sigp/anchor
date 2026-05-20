use base64::{Engine, engine::general_purpose::STANDARD};
use eth2::types::FullBlockContents;
use ssz::{Decode, DecodeError, Encode};
use tree_hash::TreeHash;
use types::{BlindedBeaconBlock, EthSpec, ForkName, Hash256};

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

/// Returns `true` if the SSZ decode error is a BLS point validation failure.
///
/// Lighthouse validates BLS during SSZ deserialization; Go's `fastssz` does not.
/// Spec fixtures may contain synthetic BLS values that are structurally valid SSZ.
pub fn is_bls_validation_error(err: &DecodeError) -> bool {
    matches!(err, DecodeError::BytesInvalid(msg) if msg.contains("BLST"))
}

/// Check whether data can be decoded as a blinded or full beacon block.
///
/// Tries blinded first, then full (mirrors Go's `GetBlockData()` order).
/// Go's `fastssz` skips BLS validation, so spec fixtures may contain synthetic BLS
/// points that Lighthouse rejects. Returns `true` if either decode succeeds or fails
/// only due to BLS validation. With the `fake_crypto` feature enabled, BLS validation
/// is skipped and blocks always decode via the success path.
pub fn can_decode_block<E: EthSpec>(data: &[u8], fork: ForkName) -> bool {
    let blinded_err = match BlindedBeaconBlock::<E>::from_ssz_bytes_for_fork(data, fork) {
        Ok(_) => return true,
        Err(e) => e,
    };

    let full_err = match FullBlockContents::<E>::from_ssz_bytes_for_fork(data, fork) {
        Ok(_) => return true,
        Err(e) => e,
    };

    is_bls_validation_error(&blinded_err) || is_bls_validation_error(&full_err)
}
