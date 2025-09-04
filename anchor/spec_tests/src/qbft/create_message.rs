use qbft::InstanceHeight;
use serde::Deserialize;
use ssv_types::{
    IndexSet, OperatorId, Round,
    consensus::{QbftMessage, QbftMessageType},
    msgid::MessageId,
};
use ssz::Decode;
use tree_hash::TreeHash;
use types::Hash256;

use super::adapters::{
    qbft::{QbftAdapter, QbftStartingState},
    spec_types::{MessageContainer, SpecTestCommitteeMember, TestSignedSSVMessage},
};
use crate::{
    QbftSpecTestType, SpecTest, SpecTestType,
    utils::deserializers::{
        deserialize_base64, deserialize_base64_option, deserialize_create_type,
        deserialize_hex_hash256,
    },
};

#[derive(Deserialize)]
pub struct CreateMessageTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Type")]
    pub test_type: String,

    #[serde(rename = "Documentation")]
    pub documentation: String,

    #[serde(rename = "Value")]
    #[serde(deserialize_with = "deserialize_hex_hash256")]
    pub root: Hash256,

    #[serde(rename = "StateValue")]
    #[serde(deserialize_with = "deserialize_base64_option")]
    pub value: Option<Vec<u8>>,

    #[serde(rename = "Round")]
    pub round: Option<u64>,

    #[serde(rename = "RoundChangeJustifications")]
    pub round_change_justifications: Option<Vec<TestSignedSSVMessage>>,

    #[serde(rename = "PrepareJustifications")]
    pub prepare_justifications: Option<Vec<TestSignedSSVMessage>>,

    #[serde(rename = "CreateType", deserialize_with = "deserialize_create_type")]
    pub msg_type: QbftMessageType,

    #[serde(rename = "ExpectedRoot")]
    #[serde(deserialize_with = "deserialize_hex_hash256")]
    pub expected_root: Hash256,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    #[serde(rename = "Identifier")]
    #[serde(deserialize_with = "deserialize_base64")]
    pub identifier: Vec<u8>,

    #[serde(rename = "CommitteeMember")]
    pub committee_member: SpecTestCommitteeMember,

    #[serde(rename = "OperatorID")]
    pub operator_id: Option<u64>,

    #[serde(skip)]
    qbft_state: Option<QbftStartingState>,
}

impl SpecTest for CreateMessageTest {
    fn setup(&mut self) {
        let committee = self.committee_member.committee.as_ref().map(|ops| {
            ops.iter()
                .map(|op| OperatorId::from(op.operator_id))
                .collect::<IndexSet<_>>()
        });

        // They all use operator 1 as the sighner
        let operator_id = OperatorId::from(1);

        let starting_state = QbftStartingState {
            height: InstanceHeight::from(0),
            identifier: MessageId::from(<[u8; 56]>::try_from(self.identifier.as_slice()).unwrap()),
            committee,
            operator_id,
            round: self.round.map(|r| Round::from(r)).unwrap_or(Round::from(1)),
            start_value: self.value.clone().unwrap_or_default(),
            proposal_accepted: None,
            propose_container: MessageContainer::default(),
            prepare_container: MessageContainer::default(),
            commit_container: MessageContainer::default(),
            round_change_container: MessageContainer::default(),
            round_change_justifications: self.round_change_justifications.clone(),
            prepare_justifications: self.prepare_justifications.clone(),
            force_stop: false,
        };

        self.qbft_state = Some(starting_state.clone());
    }

    fn run(&self) -> bool {
        let state = self
            .qbft_state
            .as_ref()
            .expect("QbftStartingState should be initialized in setup()");

        let mut adapter = QbftAdapter::new_with_state(state.clone());

        // Create the message
        let signed_ssv_message =
            adapter.create_message(self.msg_type, self.root, state.start_value.clone());

        // Compare message root to expected root
        let actual_root = signed_ssv_message.tree_hash_root();
        if actual_root != self.expected_root {
            return false;
        }

        // Validate the SignedSSVMessage
        if signed_ssv_message.validate().is_err() {
            return false;
        }

        let Ok(qbft_message) = QbftMessage::from_ssz_bytes(signed_ssv_message.ssv_message().data())
        else {
            return false;
        };

        // Validate the qbft message
        if qbft_message.validate().is_err() {
            return false;
        }

        // todo!() State comparison
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::CreateMessage)
    }
}
