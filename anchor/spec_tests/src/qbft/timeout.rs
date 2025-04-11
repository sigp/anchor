use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for TimeoutTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        println!("running");
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::Timeout)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimeoutTest {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Pre")]
    pre: Pre,
    #[serde(rename = "PostRoot")]
    post_root: String,
    #[serde(rename = "OutputMessages")]
    output_messages: Option<serde_json::Value>,
    #[serde(rename = "ExpectedTimerState")]
    expected_timer_state: ExpectedTimerState,
    #[serde(rename = "ExpectedError")]
    expected_error: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Pre {
    #[serde(rename = "State")]
    state: State,
    #[serde(rename = "StartValue")]
    start_value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    #[serde(rename = "CommitteeMember")]
    committee_member: CommitteeMember,
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Round")]
    round: u32,
    #[serde(rename = "Height")]
    height: u32,
    #[serde(rename = "LastPreparedRound")]
    last_prepared_round: u32,
    #[serde(rename = "LastPreparedValue")]
    last_prepared_value: Option<serde_json::Value>,
    #[serde(rename = "ProposalAcceptedForCurrentRound")]
    proposal_accepted_for_current_round: Option<ProposalAccepted>,
    #[serde(rename = "Decided")]
    decided: bool,
    #[serde(rename = "DecidedValue")]
    decided_value: Option<serde_json::Value>,
    #[serde(rename = "ProposeContainer")]
    propose_container: MessageContainer,
    #[serde(rename = "PrepareContainer")]
    prepare_container: MessageContainer,
    #[serde(rename = "CommitContainer")]
    commit_container: MessageContainer,
    #[serde(rename = "RoundChangeContainer")]
    round_change_container: MessageContainer,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitteeMember {
    #[serde(rename = "OperatorID")]
    operator_id: u32,
    #[serde(rename = "CommitteeID")]
    committee_id: Vec<u32>,
    #[serde(rename = "SSVOperatorPubKey")]
    ssv_operator_pub_key: String,
    #[serde(rename = "FaultyNodes")]
    faulty_nodes: u32,
    #[serde(rename = "Committee")]
    committee: Vec<CommitteeEntry>,
    #[serde(rename = "DomainType")]
    domain_type: Vec<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitteeEntry {
    #[serde(rename = "OperatorID")]
    operator_id: u32,
    #[serde(rename = "SSVOperatorPubKey")]
    ssv_operator_pub_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProposalAccepted {
    #[serde(rename = "SignedMessage")]
    signed_message: SignedMessage,
    #[serde(rename = "QBFTMessage")]
    qbft_message: QbftMessage,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignedMessage {
    #[serde(rename = "Signatures")]
    signatures: Vec<String>,
    #[serde(rename = "OperatorIDs")]
    operator_ids: Vec<u32>,
    #[serde(rename = "SSVMessage")]
    ssv_message: SsvMessage,
    #[serde(rename = "FullData")]
    full_data: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SsvMessage {
    #[serde(rename = "MsgType")]
    msg_type: u32,
    #[serde(rename = "MsgID")]
    msg_id: Vec<u8>,
    #[serde(rename = "Data")]
    data: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QbftMessage {
    #[serde(rename = "MsgType")]
    msg_type: u32,
    #[serde(rename = "Height")]
    height: u32,
    #[serde(rename = "Round")]
    round: u32,
    #[serde(rename = "Identifier")]
    identifier: String,
    #[serde(rename = "Root")]
    root: Vec<u8>,
    #[serde(rename = "DataRound")]
    data_round: u32,
    #[serde(rename = "RoundChangeJustification")]
    round_change_justification: Vec<serde_json::Value>,
    #[serde(rename = "PrepareJustification")]
    prepare_justification: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageContainer {
    #[serde(rename = "Msgs")]
    msgs: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExpectedTimerState {
    #[serde(rename = "Timeouts")]
    timeouts: u32,
    #[serde(rename = "Round")]
    round: u32,
}
