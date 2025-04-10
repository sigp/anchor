use serde::{Deserialize, Serialize};

use crate::{QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for MessageProcessingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::MessageProcessing)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageProcessingTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Pre")]
    pub pre: PreState,

    #[serde(rename = "PostRoot")]
    pub post_root: String,

    #[serde(rename = "InputMessages")]
    pub input_messages: Vec<InputMessage>,

    #[serde(rename = "OutputMessages")]
    pub output_messages: Option<Vec<InputMessage>>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    #[serde(rename = "ExpectedTimerState")]
    pub expected_timer_state: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreState {
    #[serde(rename = "State")]
    pub state: QbftState,

    #[serde(rename = "StartValue")]
    pub start_value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QbftState {
    #[serde(rename = "CommitteeMember")]
    pub committee_member: CommitteeMember,

    #[serde(rename = "ID")]
    pub id: String,

    #[serde(rename = "Round")]
    pub round: u64,

    #[serde(rename = "Height")]
    pub height: u64,

    #[serde(rename = "LastPreparedRound")]
    pub last_prepared_round: u64,

    #[serde(rename = "LastPreparedValue")]
    pub last_prepared_value: Option<String>,

    #[serde(rename = "ProposalAcceptedForCurrentRound")]
    pub proposal_accepted_for_current_round: Option<SignedProposal>,

    #[serde(rename = "Decided")]
    pub decided: bool,

    #[serde(rename = "DecidedValue")]
    pub decided_value: Option<String>,

    #[serde(rename = "ProposeContainer")]
    pub propose_container: MessageContainer,

    #[serde(rename = "PrepareContainer")]
    pub prepare_container: MessageContainer,

    #[serde(rename = "CommitContainer")]
    pub commit_container: MessageContainer,

    #[serde(rename = "RoundChangeContainer")]
    pub round_change_container: MessageContainer,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageContainer {
    #[serde(rename = "Msgs")]
    pub msgs: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignedProposal {
    #[serde(rename = "SignedMessage")]
    pub signed_message: InputMessage,

    #[serde(rename = "QBFTMessage")]
    pub qbft_message: QbftMessage,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InputMessage {
    #[serde(rename = "Signatures")]
    pub signatures: Vec<String>,

    #[serde(rename = "OperatorIDs")]
    pub operator_ids: Vec<u64>,

    #[serde(rename = "SSVMessage")]
    pub ssv_message: SsvMessage,

    #[serde(rename = "FullData")]
    pub full_data: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SsvMessage {
    #[serde(rename = "MsgType")]
    pub msg_type: u64,

    #[serde(rename = "MsgID")]
    pub msg_id: Vec<u8>,

    #[serde(rename = "Data")]
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QbftMessage {
    #[serde(rename = "MsgType")]
    pub msg_type: u64,

    #[serde(rename = "Height")]
    pub height: u64,

    #[serde(rename = "Round")]
    pub round: u64,

    #[serde(rename = "Identifier")]
    pub identifier: String,

    #[serde(rename = "Root")]
    pub root: Vec<u8>,

    #[serde(rename = "DataRound")]
    pub data_round: u64,

    #[serde(rename = "RoundChangeJustification")]
    pub round_change_justification: Vec<serde_json::Value>,

    #[serde(rename = "PrepareJustification")]
    pub prepare_justification: Vec<serde_json::Value>,
}
