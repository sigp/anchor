use serde::{Deserialize, Serialize};

use crate::{qbft::QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for QbftMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::QbftMessage)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QbftMessageTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Messages")]
    pub messages: Vec<InputMessage>,

    #[serde(rename = "EncodedMessages")]
    pub encoded_messages: Vec<String>,

    #[serde(rename = "ExpectedRoots")]
    pub expected_roots: Vec<Vec<u8>>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
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
