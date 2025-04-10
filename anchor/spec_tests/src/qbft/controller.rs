use serde::{Deserialize, Serialize};

use crate::{QbftSpecTestType, SpecTest, SpecTestType};

struct ControllerTest {
    name: String,
}

impl SpecTest for ControllerTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::Controller)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TestCase {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "RunInstanceData")]
    pub run_instance_data: Vec<RunInstance>,

    #[serde(rename = "OutputMessages")]
    pub output_messages: Option<Vec<String>>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    #[serde(rename = "omitempty", skip_serializing_if = "Option::is_none")]
    pub omitempty: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunInstance {
    #[serde(rename = "InputValue")]
    pub input_value: String,

    #[serde(rename = "InputMessages")]
    pub input_messages: Vec<InputMessage>,

    #[serde(rename = "ControllerPostRoot")]
    pub controller_post_root: String,

    #[serde(rename = "ExpectedTimerState")]
    pub expected_timer_state: Option<String>,

    #[serde(rename = "ExpectedDecidedState")]
    pub expected_decided_state: Option<DecidedState>,

    #[serde(rename = "omitempty", skip_serializing_if = "Option::is_none")]
    pub omitempty: Option<String>,
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
pub struct DecidedState {
    #[serde(rename = "DecidedVal")]
    pub decided_val: String,

    #[serde(rename = "DecidedCnt")]
    pub decided_cnt: u64,

    #[serde(rename = "BroadcastedDecided")]
    pub broadcasted_decided: Option<Vec<String>>,
}
