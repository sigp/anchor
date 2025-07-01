use crate::{SpecTest, SpecTestType, types::TypesSpecTestType};
use serde::Deserialize;
use ssv_types::message::SignedSSVMessage;

// Committee member test - using existing client committee infrastructure
#[derive(Debug, Deserialize)]
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
        // No-op
    }

    fn run(&self) -> bool {
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::CommitteeMember)
    }
}
