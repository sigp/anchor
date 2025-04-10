use crate::{QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for CreateMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::CreateMessage)
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateMessageTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Value")]
    pub value: Vec<u8>,

    #[serde(rename = "StateValue")]
    pub state_value: Option<String>,

    #[serde(rename = "Round")]
    pub round: u64,

    #[serde(rename = "RoundChangeJustifications")]
    pub round_change_justifications: Option<Vec<String>>,

    #[serde(rename = "PrepareJustifications")]
    pub prepare_justifications: Option<Vec<String>>,

    #[serde(rename = "CreateType")]
    pub create_type: CreateType,

    #[serde(rename = "ExpectedRoot")]
    pub expected_root: String,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CreateType {
    #[serde(rename = "CreatePrepare")]
    Prepare,

    #[serde(rename = "CreateCommit")]
    Commit,

    #[serde(rename = "CreateRoundChange")]
    RoundChange,
}
