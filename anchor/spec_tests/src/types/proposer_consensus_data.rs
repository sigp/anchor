use serde::Deserialize;
use ssv_types::consensus::{BEACON_ROLE_PROPOSER, ProposerConsensusData};
use types::{ForkName, MainnetEthSpec};

use crate::{
    SpecTest,
    utils::{can_decode_block, dtos::RawConsensusData, error_codes},
};

/// Mirrors Go's `ConsensusData.Validate()` for proposer duties.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProposerConsensusDataTest {
    consensus_data: RawConsensusData,
    expected_error_code: i64,
}

impl SpecTest for ProposerConsensusDataTest {
    fn run(&self) -> Result<(), String> {
        let actual_code = match self.validate() {
            Ok(()) => error_codes::NO_ERROR,
            Err(code) => code,
        };
        error_codes::assert_error_code(self.expected_error_code, actual_code)
    }
}

impl ProposerConsensusDataTest {
    fn validate(&self) -> Result<(), i64> {
        let proposer_data = ProposerConsensusData::try_from(&self.consensus_data)
            .map_err(|_| error_codes::UNKNOWN_DUTY_ROLE_DATA)?;

        if proposer_data.duty.r#type != BEACON_ROLE_PROPOSER {
            return Err(error_codes::UNKNOWN_DUTY_ROLE_DATA);
        }

        let fork: ForkName = proposer_data.version.into();
        if can_decode_block::<MainnetEthSpec>(&proposer_data.data_ssz, fork) {
            Ok(())
        } else {
            Err(error_codes::UNMARSHAL_SSZ)
        }
    }
}
