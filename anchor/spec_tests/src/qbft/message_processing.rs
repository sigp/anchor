use qbft::InstanceHeight;
use serde::Deserialize;
use ssv_types::message::SignedSSVMessage;
use ssv_types::{IndexSet, OperatorId, Round, msgid::MessageId};
use tree_hash::TreeHash;

use super::adapters::{
    qbft::{QbftAdapter, QbftStartingState},
    spec_types::{
        AcceptedProposal, ExpectedTimerState, MessageContainer, SpecTestCommitteeMember,
        TestSignedSSVMessage,
    },
};
use crate::{
    QbftSpecTestType, SpecTest, SpecTestType,
    utils::deserializers::{deserialize_base64, deserialize_base64_option},
};

#[derive(Debug, Clone, Deserialize)]
pub struct MessageProcessingTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Type")]
    pub test_type: String,

    #[serde(rename = "Documentation")]
    pub documentation: String,

    #[serde(rename = "Pre")]
    pub pre: MessageProcessingPre,

    #[serde(rename = "PostRoot", deserialize_with = "deserialize_base64_option")]
    pub post_root: Option<Vec<u8>>,

    #[serde(rename = "InputMessages")]
    pub input_messages: Vec<TestSignedSSVMessage>,

    #[serde(rename = "OutputMessages")]
    pub output_messages: Option<Vec<TestSignedSSVMessage>>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    #[serde(rename = "ExpectedTimerState")]
    pub expected_timer_state: Option<ExpectedTimerState>,

    #[serde(skip)]
    qbft_state: Option<QbftStartingState>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageProcessingPre {
    #[serde(rename = "forceStop")]
    pub force_stop: Option<bool>,

    #[serde(rename = "State")]
    pub state: MessageProcessingState,

    #[serde(rename = "StartValue", deserialize_with = "deserialize_base64")]
    pub start_value: Vec<u8>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageProcessingState {
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

impl SpecTest for MessageProcessingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Build the starting state
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
            start_value: self.pre.start_value.clone(),
            proposal_accepted: self.pre.state.proposal_accepted_for_current_round.clone(),
            propose_container: self.pre.state.propose_container.clone(),
            prepare_container: self.pre.state.prepare_container.clone(),
            commit_container: self.pre.state.commit_container.clone(),
            round_change_container: self.pre.state.round_change_container.clone(),
            round_change_justifications: None,
            prepare_justifications: None,
            force_stop: self.pre.force_stop.unwrap_or(false),
        };

        self.qbft_state = Some(state);
    }

    fn run(&self) -> bool {
        let state = self
            .qbft_state
            .as_ref()
            .expect("QbftStartingState should be initialized in setup()");

        let mut adapter = QbftAdapter::new_with_state(state.clone());

        // Process each input message
        let mut last_error = None;
        for msg in self.input_messages.iter() {
            if let Err(e) = adapter.process_message(msg) {
                last_error = Some(e);
            }
        }

        // Check error expectations
        if !self.expected_error.is_empty() {
            match last_error {
                Some(e) => {
                    // make sure the errors match
                    if e != self.expected_error {
                        return false;
                    }
                }
                None => {
                    return false;
                }
            }
        } else if let Some(_) = last_error {
            // Got an error when one was not expected
            return false;
        }

        // Check output messages
        if let Some(expected_msgs) = &self.output_messages {
            let captured = adapter.get_captured_messages();
            if captured.len() != expected_msgs.len() {
                return false;
            }

            for (captured_msg, expected_msg) in captured.iter().zip(expected_msgs) {
                let expected_signed: SignedSSVMessage = expected_msg.clone().try_into().unwrap();
                if captured_msg.tree_hash_root() != expected_signed.tree_hash_root() {
                    return false;
                }
            }
        }

        // TODO: Check post-state root (same JSON issues as timeout tests)

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::MsgProcessing)
    }
}
