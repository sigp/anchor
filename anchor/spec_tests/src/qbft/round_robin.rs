use serde::{Deserialize, Serialize};

use crate::{QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for RoundRobinTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::RoundRobin)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoundRobinTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Share")]
    pub share: CommitteeMember,

    #[serde(rename = "Heights")]
    pub heights: Vec<u64>,

    #[serde(rename = "Rounds")]
    pub rounds: Vec<u64>,

    #[serde(rename = "Proposers")]
    pub proposers: Vec<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitteeMember {
    #[serde(rename = "OperatorID")]
    pub operator_id: u64,

    #[serde(rename = "CommitteeID")]
    pub committee_id: Vec<u8>,

    #[serde(rename = "SSVOperatorPubKey")]
    pub ssv_operator_pub_key: String,

    #[serde(rename = "FaultyNodes")]
    pub faulty_nodes: u64,

    #[serde(rename = "Committee")]
    pub committee: Vec<CommitteeMemberInfo>,

    #[serde(rename = "DomainType")]
    pub domain_type: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitteeMemberInfo {
    #[serde(rename = "OperatorID")]
    pub operator_id: u64,

    #[serde(rename = "SSVOperatorPubKey")]
    pub ssv_operator_pub_key: String,
}
