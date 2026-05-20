use serde::Deserialize;
use ssv_types::consensus::ProposerConsensusData;
use ssz::{Decode, DecodeError, Encode};
use tree_hash::TreeHash;
use types::{Hash256, MainnetEthSpec};

use crate::{
    SpecTest,
    utils::{
        deserializers::{deserialize_base64, deserialize_base64_or_null, deserialize_hex_hash256},
        error_codes, is_bls_validation_error,
    },
};

/// Mirrors Go's `ProposerSpecTest.Run()`: SSZ-decode consensus data, extract block,
/// then verify blinded state, roots, and SSZ roundtrips.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ConsensusDataProposerTest {
    #[serde(rename = "DataCd", deserialize_with = "deserialize_base64")]
    consensus_data_ssz: Vec<u8>,

    #[serde(
        rename = "DataBlk",
        deserialize_with = "deserialize_base64_or_null",
        default
    )]
    expected_block_ssz: Option<Vec<u8>>,

    blinded: bool,

    #[serde(
        rename = "ExpectedBlkRoot",
        deserialize_with = "deserialize_hex_hash256"
    )]
    expected_block_root: Hash256,

    #[serde(
        rename = "ExpectedCdRoot",
        deserialize_with = "deserialize_hex_hash256"
    )]
    expected_consensus_data_root: Hash256,

    expected_error_code: i64,
}

impl SpecTest for ConsensusDataProposerTest {
    fn run(&self) -> Result<(), String> {
        // Decode ProposerConsensusData from SSZ
        let proposer_data = match ProposerConsensusData::from_ssz_bytes(&self.consensus_data_ssz) {
            Ok(data) => data,
            Err(DecodeError::NoMatchingVariant) => {
                return self.expect_error(error_codes::UNKNOWN_BLOCK_VERSION);
            }
            Err(_) => return self.expect_error(error_codes::UNMARSHAL_SSZ),
        };

        // Decode block — try blinded first, then full (mirrors Go's GetBlockData)
        let blinded_err = match proposer_data.decode_blinded_block::<MainnetEthSpec>() {
            Ok(blinded_block) => {
                self.expect_success()?;
                self.verify_block(
                    true,
                    blinded_block.tree_hash_root(),
                    &blinded_block.as_ssz_bytes(),
                )?;
                return self.verify_consensus_data(&proposer_data);
            }
            Err(e) => e,
        };

        let full_err = match proposer_data.decode_block_contents::<MainnetEthSpec>() {
            Ok(full_contents) => {
                self.expect_success()?;
                // Go's `vBlk.Root()` returns the inner `BeaconBlock` root, but
                // `vBlk.Deneb.MarshalSSZ()` encodes the full block contents (block + blobs).
                // `FullBlockContents` doesn't implement `Encode`, so we compare against the
                // raw `data_ssz` bytes from `ProposerConsensusData` which is what was decoded.
                let inner_block = full_contents.block();
                self.verify_block(false, inner_block.tree_hash_root(), &proposer_data.data_ssz)?;
                return self.verify_consensus_data(&proposer_data);
            }
            Err(e) => e,
        };

        // Both decoders failed. Go fixtures use synthetic BLS points that Go's fastssz
        // accepts but Lighthouse rejects (`BLST_BAD_ENCODING`). Without `fake_crypto`,
        // all success fixtures hit this path and `verify_block()` is never reached — only
        // consensus data root/SSZ verification runs. With `fake_crypto` enabled
        // (`cargo test -p spec_tests --features fake_crypto`), BLS validation is skipped,
        // blocks decode successfully above, and `verify_block()` provides full coverage
        // matching Go's `ProposerSpecTest.Run()`.
        let blinded_bls_error = is_bls_validation_error(&blinded_err);
        if blinded_bls_error || is_bls_validation_error(&full_err) {
            self.expect_success()?;
            // If blinded decode reached BLS validation, the block is structurally blinded.
            if blinded_bls_error != self.blinded {
                return Err(format!(
                    "Block blinded state mismatch (BLS path): got {blinded_bls_error}, expected {}",
                    self.blinded,
                ));
            }
            return self.verify_consensus_data(&proposer_data);
        }

        self.expect_error(error_codes::UNMARSHAL_SSZ)
    }
}

impl ConsensusDataProposerTest {
    /// Check that expected_error_code is NO_ERROR (decode succeeded).
    fn expect_success(&self) -> Result<(), String> {
        if self.expected_error_code != error_codes::NO_ERROR {
            Err(format!(
                "Expected error code {}, but block extraction succeeded",
                self.expected_error_code,
            ))
        } else {
            Ok(())
        }
    }

    /// Verify block blinded state, root, and SSZ roundtrip.
    fn verify_block(
        &self,
        is_blinded: bool,
        block_root: Hash256,
        block_ssz: &[u8],
    ) -> Result<(), String> {
        if is_blinded != self.blinded {
            return Err(format!(
                "Block blinded state mismatch: got {is_blinded}, expected {}",
                self.blinded,
            ));
        }

        if block_root != self.expected_block_root {
            return Err(format!(
                "Block root mismatch: got {block_root}, expected {}",
                self.expected_block_root,
            ));
        }

        let expected = self
            .expected_block_ssz
            .as_ref()
            .ok_or("Expected DataBlk for success case, but got null")?;
        if block_ssz != expected.as_slice() {
            return Err(format!(
                "Block SSZ roundtrip mismatch: got {} bytes, expected {} bytes",
                block_ssz.len(),
                expected.len(),
            ));
        }

        Ok(())
    }

    /// Verify consensus data hash tree root and SSZ roundtrip.
    fn verify_consensus_data(&self, proposer_data: &ProposerConsensusData) -> Result<(), String> {
        let root = proposer_data.tree_hash_root();
        if root != self.expected_consensus_data_root {
            return Err(format!(
                "ConsensusData root mismatch: got {root}, expected {}",
                self.expected_consensus_data_root,
            ));
        }

        let re_encoded = proposer_data.as_ssz_bytes();
        if re_encoded != self.consensus_data_ssz {
            return Err(format!(
                "ConsensusData SSZ roundtrip mismatch: re-encoded {} bytes, original {} bytes",
                re_encoded.len(),
                self.consensus_data_ssz.len(),
            ));
        }

        Ok(())
    }

    /// Check that the actual error code matches the expected one.
    fn expect_error(&self, actual_code: i64) -> Result<(), String> {
        if actual_code != self.expected_error_code {
            Err(format!(
                "Expected error code {}, got {actual_code}",
                self.expected_error_code,
            ))
        } else {
            Ok(())
        }
    }
}
