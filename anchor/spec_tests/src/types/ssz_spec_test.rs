use serde::Deserialize;
use ssz::Decode;
use types::{ExecPayload, Hash256, MainnetEthSpec};

use crate::{
    SpecTest,
    utils::{
        deserializers::{deserialize_base64, deserialize_hex_hash256},
        is_bls_validation_error,
    },
};

/// Mirrors Go's `SSZSpecTest.Run()`. Verifies the withdrawals root from the
/// execution payload when block decode succeeds.
///
/// Fixtures contain synthetic BLS; Lighthouse rejects these during SSZ decode
/// (Go's `fastssz` doesn't validate BLS), so withdrawals root verification is
/// only exercised with `fake_crypto` enabled.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SSZSpecTest {
    #[serde(deserialize_with = "deserialize_base64")]
    data: Vec<u8>,
    #[serde(deserialize_with = "deserialize_hex_hash256")]
    expected_root: Hash256,
}

impl SpecTest for SSZSpecTest {
    fn run(&self) -> Result<(), String> {
        let cd = ssv_types::consensus::ProposerConsensusData::from_ssz_bytes(&self.data)
            .map_err(|e| format!("Failed to decode ProposerConsensusData: {e:?}"))?;

        // Try blinded then full (same order as Go).
        let withdrawals_root = match cd.decode_blinded_block::<MainnetEthSpec>() {
            Ok(blinded) => blinded
                .body()
                .execution_payload()
                .and_then(|p| p.withdrawals_root())
                .map_err(|e| format!("Failed to get withdrawals root from blinded block: {e:?}"))?,
            Err(blinded_err) => match cd.decode_block_contents::<MainnetEthSpec>() {
                Ok(full) => full
                    .block()
                    .body()
                    .execution_payload()
                    .and_then(|p| p.withdrawals_root())
                    .map_err(|e| {
                        format!("Failed to get withdrawals root from full block: {e:?}")
                    })?,
                // Both decoders failed. Go fixtures use synthetic BLS points that Go's
                // fastssz accepts but Lighthouse rejects (`BLST_BAD_ENCODING`). Without
                // `fake_crypto`, the withdrawals root check is unreachable and this
                // returns `Ok(())` to preserve parity with the current Go `SSZSpecTest`.
                // With `fake_crypto` enabled
                // (`cargo test -p spec_tests --features fake_crypto`), BLS validation is
                // skipped, the full block decodes successfully above, and the withdrawals
                // root is actually verified. Tracked by ssvlabs/ssv-spec#622.
                Err(full_err) => {
                    if is_bls_validation_error(&blinded_err) || is_bls_validation_error(&full_err) {
                        return Ok(());
                    }
                    return Err(format!(
                        "Both block decoders failed: blinded={blinded_err:?}, full={full_err:?}"
                    ));
                }
            },
        };

        if withdrawals_root != self.expected_root {
            return Err(format!(
                "Withdrawals root mismatch: got {withdrawals_root}, expected {}",
                self.expected_root,
            ));
        }

        Ok(())
    }
}
