use indexmap::IndexSet;
use qbft::{DefaultLeaderFunction, InstanceHeight, LeaderFunction};
use serde::Deserialize;
use ssv_types::{OperatorId, Round};

use super::adapters::spec_types::SpecTestCommitteeMember;
use crate::{QbftSpecTestType, SpecTest, SpecTestType};

#[derive(Debug, Clone, Deserialize)]
pub struct RoundRobinTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Type")]
    pub test_type: String,

    #[serde(rename = "Documentation")]
    pub documentation: String,

    #[serde(rename = "Share")]
    pub share: SpecTestCommitteeMember,

    #[serde(rename = "Heights")]
    pub heights: Vec<u64>,

    #[serde(rename = "Rounds")]
    pub rounds: Vec<u64>,

    #[serde(rename = "Proposers")]
    pub proposers: Vec<u64>,
}

/// Round-robin proposer selection algorithm using DefaultLeaderFunction
fn round_robin_proposer(committee: &[OperatorId], height: u64, round: u64) -> OperatorId {
    let leader_fn = DefaultLeaderFunction::default();
    let committee_set: IndexSet<OperatorId> = committee.iter().copied().collect();
    let round = Round::from(round);
    let instance_height = InstanceHeight::from(height as usize);

    // Find the proposer by testing each committee member
    for member in committee {
        if leader_fn.leader_function(member, round, instance_height, &committee_set) {
            return *member;
        }
    }
    unreachable!("One committee member must be the leader")
}

impl SpecTest for RoundRobinTest {
    fn run(&self) -> bool {
        let committee_ids: Vec<OperatorId> = self
            .share
            .committee
            .as_ref()
            .map(|ops| {
                ops.iter()
                    .map(|member| OperatorId::from(member.operator_id))
                    .collect()
            })
            .unwrap_or_default();

        for i in 0..self.heights.len() {
            let height = self.heights[i];
            let round = self.rounds[i];
            let expected_proposer = OperatorId::from(self.proposers[i]);

            let actual_proposer = round_robin_proposer(&committee_ids, height, round);

            if actual_proposer != expected_proposer {
                return false;
            }
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::RoundRobin)
    }
}
