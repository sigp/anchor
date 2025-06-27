use serde::Deserialize;
use ssv_types::message::SignedSSVMessage;

use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};

// Notes:
// Same as beacon vote, the test file and the comparison are the same
// They do not actually use it and use hardcoded data instead They do not actually use it and use
// hardcoded data instead...
// https://github.com/ssvlabs/ssv-spec/blob/4faf15cc6598254f2b4602b094f08cc7aa3c74ef/types/spectest/tests/committeemember/has_quorum.go#L11

// Committee member test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitteeMemberTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "CommitteeMember")]
    pub committee_member: serde_json::Value,
    #[serde(rename = "Message")]
    pub message: SignedSSVMessage,
    #[serde(rename = "ExpectedHasQuorum")]
    pub expected_has_quorum: bool,
    #[serde(rename = "ExpectedFullCommittee")]
    pub expected_full_committee: bool,
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for CommitteeMemberTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::CommitteeMember)
    }
}
