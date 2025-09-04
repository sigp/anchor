use qbft::InstanceHeight;
use serde::Deserialize;
use ssv_types::{IndexSet, OperatorId, Round, message::SignedSSVMessage, msgid::MessageId};
use tree_hash::TreeHash;
use types::Hash256;

use super::adapters::{
    qbft::{QbftAdapter, QbftStartingState},
    spec_types::{
        AcceptedProposal, ExpectedTimerState, MessageContainer, SpecTestCommitteeMember,
        TestSignedSSVMessage,
    },
};
use crate::{
    QbftSpecTestType, SpecTest, SpecTestType,
    utils::{
        deserializers::{deserialize_base64, deserialize_base64_option, deserialize_hex_hash256},
        test_keys::TestKeySet,
    },
};

#[derive(Debug, Clone, Deserialize)]
pub struct TimeoutTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Type")]
    pub test_type: String,

    #[serde(rename = "Documentation")]
    pub documentation: String,

    #[serde(rename = "Pre")]
    pub pre: TimeoutTestPre,

    #[serde(rename = "PostRoot", deserialize_with = "deserialize_hex_hash256")]
    pub post_root: Hash256,

    #[serde(rename = "OutputMessages")]
    pub output_messages: Option<Vec<TestSignedSSVMessage>>,

    #[serde(rename = "ExpectedTimerState")]
    pub expected_timer_state: Option<ExpectedTimerState>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    #[serde(skip)]
    qbft_state: Option<QbftStartingState>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimeoutTestPre {
    #[serde(rename = "State")]
    pub state: QbftInstanceState,

    #[serde(rename = "StartValue", deserialize_with = "deserialize_base64_option")]
    pub start_value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QbftInstanceState {
    #[serde(rename = "CommitteeMember")]
    pub committee_member: SpecTestCommitteeMember,

    #[serde(rename = "ID", deserialize_with = "deserialize_base64")]
    pub id: Vec<u8>,

    #[serde(rename = "Round")]
    pub round: u64,

    #[serde(rename = "Height")]
    pub height: u64,

    #[serde(rename = "LastPreparedRound")]
    pub last_prepared_round: u64,

    #[serde(
        rename = "LastPreparedValue",
        deserialize_with = "deserialize_base64_option"
    )]
    pub last_prepared_value: Option<Vec<u8>>,

    #[serde(rename = "ProposalAcceptedForCurrentRound")]
    pub proposal_accepted_for_current_round: Option<AcceptedProposal>,

    #[serde(rename = "Decided")]
    pub decided: bool,

    #[serde(
        rename = "DecidedValue",
        deserialize_with = "deserialize_base64_option"
    )]
    pub decided_value: Option<Vec<u8>>,

    #[serde(rename = "ProposeContainer")]
    pub propose_container: MessageContainer,

    #[serde(rename = "PrepareContainer")]
    pub prepare_container: MessageContainer,

    #[serde(rename = "CommitContainer")]
    pub commit_container: MessageContainer,

    #[serde(rename = "RoundChangeContainer")]
    pub round_change_container: MessageContainer,
}

impl SpecTest for TimeoutTest {
    fn setup(&mut self) {
        // Build the starting state from timeout test pre
        let committee: IndexSet<OperatorId> = self
            .pre
            .state
            .committee_member
            .committee
            .as_ref()
            .map(|ops| {
                ops.iter()
                    .map(|op| OperatorId::from(op.operator_id))
                    .collect()
            })
            .unwrap_or_default();

        let state = QbftStartingState {
            height: InstanceHeight::from(self.pre.state.height as usize),
            identifier: MessageId::from(
                <[u8; 56]>::try_from(self.pre.state.id.as_slice()).unwrap(),
            ),
            committee: Some(committee),
            operator_id: OperatorId::from(self.pre.state.committee_member.operator_id),
            round: Round::from(self.pre.state.round),
            start_value: self.pre.start_value.clone().unwrap(),
            proposal_accepted: self.pre.state.proposal_accepted_for_current_round.clone(),
            propose_container: self.pre.state.propose_container.clone(),
            prepare_container: self.pre.state.prepare_container.clone(),
            commit_container: self.pre.state.commit_container.clone(),
            round_change_container: self.pre.state.round_change_container.clone(),
            round_change_justifications: None,
            prepare_justifications: None,
            force_stop: false,
        };

        self.qbft_state = Some(state);
    }

    fn run(&self) -> bool {
        // Use the state constructed in setup()
        let state = self
            .qbft_state
            .as_ref()
            .expect("QbftStartingState should be initialized in setup()");

        // Create adapter with state
        let mut adapter = QbftAdapter::new_with_state(state.clone());

        // Record initial round
        let initial_round = adapter.get_round();

        // Trigger timeout and handle potential error
        match adapter.trigger_timeout() {
            Ok(()) => {
                // Timeout succeeded - check if we expected an error
                if !self.expected_error.is_empty() {
                    return false;
                }
            }
            Err(err) => {
                // Timeout returned an error - check if it matches expected
                if self.expected_error.is_empty() || err != self.expected_error {
                    return false;
                }

                // For error cases , verify state unchanged
                if adapter.get_round() != initial_round {
                    return false;
                }

                // Verify no messages were sent
                if !adapter.get_captured_messages().is_empty() {
                    return false;
                }

                // Error case handled correctly
                return true;
            }
        }

        // Make sure round was incremented
        let new_round = adapter.get_round();
        if new_round != initial_round + 1 {
            return false;
        }

        // Check timer state if provided
        if let Some(expected_timer) = &self.expected_timer_state {
            // Validate the round if specified
            if let Some(expected_round) = expected_timer.round {
                if new_round != expected_round {
                    return false;
                }
            }

            // Validate the timeout count
            let timeout_count = adapter.get_timeout_count();
            if timeout_count != expected_timer.timeouts {
                return false;
            }
        }

        // Check output messages
        let captured = adapter.get_captured_messages();
        let test_keys = TestKeySet::four_share_set();
        if test_keys.verify_signed_messages(&captured).is_err() {
            return false;
        }

        if let Some(expected_msgs) = &self.output_messages {
            if captured.len() != expected_msgs.len() {
                return false;
            }

            // Validate each message matches expected by comparing roots
            for (captured_msg, expected_msg) in captured.iter().zip(expected_msgs.iter()) {
                let expected_msg: SignedSSVMessage =
                    expected_msg.clone().try_into().expect("Valid Message");
                if captured_msg.tree_hash_root() != expected_msg.tree_hash_root() {
                    return false;
                }
            }
        }

        // TODO: Post-state root validation

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::Timeout)
    }
}
