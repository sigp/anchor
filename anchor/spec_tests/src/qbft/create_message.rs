use std::{collections::VecDeque, sync::Arc};

use parking_lot::RwLock;
use qbft::UnsignedWrappedQbftMessage;
use serde::Deserialize;
use ssv_types::{consensus::QbftMessageType, Round};
use types::Hash256;

use super::{qbft_deserializers::*, SpecQbft};
use crate::{QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for CreateMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    // Run the test by constructing the message and verifying its correctness
    fn run(&self) -> bool {
        let spec_qbft = self.spec_qbft.as_ref().expect("Setup has been called");

        // Create a new SignedSSVMessage given the test setup
        let signed_message = spec_qbft.create_message(self.create_type);

        // Compute the merkle root of the message and compare it to the expected_root
        spec_qbft.verify_root(signed_message, self.expected_root)
    }

    // Setup the qbft instance for constructing a new message
    fn setup(&mut self) {
        let (qbft, queue) = SpecQbft::new();

        // Complete the setup
        self.spec_qbft = Some(qbft);
        self.msg_rx = Some(queue);
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::CreateMessage)
    }
}

#[derive(Deserialize)]
pub struct CreateMessageTest {
    // Name of the test that is being run
    #[serde(rename = "Name")]
    pub name: String,

    // Root of the QBFT Message, This is the unhashed ssz bytes of the data
    #[serde(rename = "Value", deserialize_with = "deserialize_value_into_root")]
    pub root: Hash256,

    // The last prepared value of the qbft instance. Todo!() What format is this in??
    #[serde(rename = "StateValue")]
    pub state_value: Option<String>,

    // The round this message is for
    #[serde(rename = "Round", deserialize_with = "deserialize_u64_into_round")]
    pub round: Option<Round>,

    // Any round change justifications for the message
    #[serde(rename = "RoundChangeJustifications")]
    pub round_change_justifications: Option<Vec<Justification>>,

    // Any prepare justifications for the message
    #[serde(rename = "PrepareJustifications")]
    pub prepare_justifications: Option<Vec<Justification>>,

    // The type of the QBFT Message to create
    #[serde(
        rename = "CreateType",
        deserialize_with = "deserialize_qbft_message_type"
    )]
    pub create_type: QbftMessageType,

    // The Expected Root of the QBFT Message
    #[serde(rename = "ExpectedRoot")]
    pub expected_root: Hash256,

    // Any Errors that were expected
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    // Qbft Instance that is used for running the test. Skip this during deserialization
    #[serde(skip)]
    pub spec_qbft: Option<SpecQbft>,

    // Unsigned message receiver
    #[serde(skip)]
    pub msg_rx: Option<Arc<RwLock<VecDeque<UnsignedWrappedQbftMessage>>>>,
}

#[derive(Debug, Deserialize)]
pub struct Justification {
    #[serde(rename = "Signatures")]
    pub signatures: Vec<String>,
    #[serde(rename = "OperatorIDs")]
    pub operator_ids: Vec<u64>,
    #[serde(rename = "SSVMessage")]
    pub ssv_message: SsvMessage,
    #[serde(rename = "FullData")]
    pub full_data: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SsvMessage {
    #[serde(rename = "MsgType")]
    pub msg_type: u8,
    #[serde(rename = "MsgID")]
    pub msg_id: Vec<u8>,
    #[serde(rename = "Data")]
    pub data: String,
}
