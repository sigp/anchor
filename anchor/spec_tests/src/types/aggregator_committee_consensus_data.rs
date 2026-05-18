use serde::Deserialize;
use ssv_types::consensus::AggregatorCommitteeDataValidator;
use types::MainnetEthSpec;

use crate::{
    SpecTest,
    utils::{
        dtos::RawAggregatorCommitteeConsensusData,
        error_codes::{self, aggregator_committee_validation_error_code},
    },
};

/// Mirrors Go's `AggregatorCommitteeConsensusData.Validate()`.
///
/// Exercises the production `AggregatorCommitteeDataValidator::do_validation` so any
/// drift between Anchor's validator and ssv-spec's is caught by the fixtures.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AggregatorCommitteeConsensusDataTest {
    #[serde(rename = "AggCommConsensusData")]
    consensus_data: RawAggregatorCommitteeConsensusData,
    expected_error_code: i64,
}

impl SpecTest for AggregatorCommitteeConsensusDataTest {
    fn run(&self) -> Result<(), String> {
        let actual_code = match self.validate() {
            Ok(()) => error_codes::NO_ERROR,
            Err(code) => code,
        };
        error_codes::assert_error_code(self.expected_error_code, actual_code)
    }
}

impl AggregatorCommitteeConsensusDataTest {
    fn validate(&self) -> Result<(), i64> {
        let consensus_data =
            ssv_types::consensus::AggregatorCommitteeConsensusData::<MainnetEthSpec>::try_from(
                &self.consensus_data,
            )
            .map_err(|_| error_codes::UNMAPPED_ERROR_CODE)?;

        AggregatorCommitteeDataValidator::<MainnetEthSpec>::default()
            .do_validation(&consensus_data)
            .map_err(aggregator_committee_validation_error_code)
    }
}
