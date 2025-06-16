mod controller;
mod create_message;
mod message_processing;
mod qbft_message;
mod round_robin;
mod timeout;

use std::{collections::VecDeque, sync::Arc};

pub use create_message::CreateMessageTest;
use openssl::{
    hash::MessageDigest,
    pkey::{PKey, Private},
    sign::Signer,
};
use parking_lot::RwLock;
use qbft::{
    Config, ConfigBuilder, DefaultLeaderFunction, InstanceHeight, Qbft, UnsignedWrappedQbftMessage,
};
use serde::{Deserialize, Deserializer};
use ssv_types::{
    IndexSet, OperatorId, Round,
    consensus::{BeaconVote, QbftMessageType},
    message::SignedSSVMessage,
    msgid::MessageId,
};
use ssz::Encode;
pub use timeout::TimeoutTest;
use tree_hash::TreeHash;
use types::Hash256;

// Convenient type wrapper
pub type QbftSendFn = Box<dyn FnMut(UnsignedWrappedQbftMessage) + Send + Sync>;
pub type ExplicitQbft = Qbft<DefaultLeaderFunction, BeaconVote, QbftSendFn>;
pub type ExplicitSendFn = Arc<RwLock<VecDeque<UnsignedWrappedQbftMessage>>>;

// Wrapper type around a QBFT instance that allows us to crate spec testing specific functions
pub struct SpecQbft(pub ExplicitQbft);
impl SpecQbft {
    // Construct a wrapped qbft instance
    pub fn new(committee: IndexSet<OperatorId>, identifier: MessageId) -> Self {
        let config: Config<DefaultLeaderFunction> =
            ConfigBuilder::new(1.into(), InstanceHeight::default(), committee)
                .build()
                .unwrap();

        // Todo!(). For creation tests, start data does not matter since we are not testing
        // consensus. Adjust for consensus tests
        let data = BeaconVote {
            block_root: Hash256::random(),
            source: types::Checkpoint::default(),
            target: types::Checkpoint::default(),
        };

        let msg_queue = Arc::new(RwLock::new(VecDeque::new()));
        let msg_queue_clone = msg_queue.clone();

        let message_handler: QbftSendFn = Box::new(move |message| {
            msg_queue_clone.write().push_back(message);
        });

        let qbft = Qbft::new(config, data, identifier, message_handler);

        SpecQbft(qbft)
    }

    // Create a new UnsignedSSVMessage. Will be send to the queue registered with the qbft instance
    pub fn create_message(
        &self,
        message_type: QbftMessageType,
        data_hash: Hash256,
        round: Option<Round>,
        round_change_justifications: Vec<SignedSSVMessage>,
        prepare_justifications: Vec<SignedSSVMessage>,
    ) -> UnsignedWrappedQbftMessage {
        self.0.new_unsigned_message_spec(
            message_type,
            data_hash,
            round_change_justifications,
            prepare_justifications,
            round,
        )
    }

    // In favor of not having to construct an entire NetworkMessageSender, just copy the signing
    // code
    fn sign(
        &self,
        unsigned: UnsignedWrappedQbftMessage,
        private_key: &PKey<Private>,
    ) -> SignedSSVMessage {
        let serialized = unsigned.unsigned_message.ssv_message.as_ssz_bytes();
        let mut signer = Signer::new(MessageDigest::sha256(), private_key).expect("Valid signer");
        signer
            .update(&serialized)
            .expect("Serialized data is valid");
        let signature = signer.sign_to_vec().expect("Signature is valid");

        SignedSSVMessage::new(
            vec![signature],
            vec![OperatorId::from(1)], // todo!() do we pass this in??
            unsigned.unsigned_message.ssv_message,
            unsigned.unsigned_message.full_data,
        )
        .expect("Data is valid")
    }

    // Confirm that merkle root of signed message equals the expected root
    pub fn verify_root(&self, msg: SignedSSVMessage, root: Hash256) -> bool {
        msg.tree_hash_root() == root
    }
}

#[derive(Eq, PartialEq, Hash)]
pub(crate) enum QbftSpecTestType {
    Timeout,
    QbftMessage,
    MessageProcessing,
    CreateMessage,
    Controller,
    RoundRobin,
}

// Contains specific identifier for the test file
impl std::fmt::Display for QbftSpecTestType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            QbftSpecTestType::Timeout => write!(f, "timeout"),
            QbftSpecTestType::QbftMessage => write!(f, "MsgSpecTest"),
            QbftSpecTestType::MessageProcessing => write!(f, "MsgProcessingSpecTest"),
            QbftSpecTestType::CreateMessage => write!(f, "CreateMsgSpecTest"),
            QbftSpecTestType::Controller => write!(f, "ControllerSpecTest"),
            QbftSpecTestType::RoundRobin => write!(f, "RoundRobinSpecTest"),
        }
    }
}

// Custom QBFT Specific serde deserializers
pub(crate) mod qbft_deserializers {
    use super::*;

    // Convert from string into QbftMessageType
    pub(crate) fn deserialize_qbft_message_type<'de, D>(
        deserializer: D,
    ) -> Result<QbftMessageType, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "createProposal" => Ok(QbftMessageType::Proposal),
            "CreatePrepare" => Ok(QbftMessageType::Prepare),
            "CreateCommit" => Ok(QbftMessageType::Commit),
            "CreateRoundChange" => Ok(QbftMessageType::RoundChange),
            _ => Err(serde::de::Error::custom(format!(
                "Invalid message type: {}",
                s
            ))),
        }
    }

    // The root of the QBFT message is passed in as the ssz bytes of the data. We need to hash this
    // and convert it it into a Hash256
    pub(crate) fn deserialize_value_into_root<'de, D>(deserializer: D) -> Result<Hash256, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Retrieve the bytes...
        let bytes = <Vec<u8>>::deserialize(deserializer)?;
        Ok(Hash256::from_slice(bytes.as_slice()))
    }

    // Convert from u64 into Round
    pub(crate) fn deserialize_u64_into_round<'de, D>(
        deserializer: D,
    ) -> Result<Option<Round>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let round = <u64>::deserialize(deserializer)?;
        if round == 0 {
            Ok(None)
        } else {
            Ok(Some(round.into()))
        }
    }
}
