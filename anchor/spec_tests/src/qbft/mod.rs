mod controller;
mod create_message;
mod message_processing;
mod qbft_message;
mod round_robin;
mod timeout;

use std::{collections::VecDeque, sync::Arc};

pub use create_message::CreateMessageTest;
use parking_lot::RwLock;
use qbft::{
    Config, ConfigBuilder, DefaultLeaderFunction, InstanceHeight, Qbft, UnsignedWrappedQbftMessage,
};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use ssv_types::{
    consensus::{BeaconVote, QbftMessageType, UnsignedSSVMessage},
    message::SignedSSVMessage,
    msgid::MessageId,
    OperatorId, Round,
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
    pub fn new() -> (Self, ExplicitSendFn) {
        let config: Config<DefaultLeaderFunction> = ConfigBuilder::new(
            1.into(),
            InstanceHeight::default(),
            (1..=4).map(OperatorId::from).collect(),
        )
        .build()
        .unwrap();

        let data = BeaconVote {
            block_root: Hash256::random(),
            source: types::Checkpoint::default(),
            target: types::Checkpoint::default(),
        };

        let msg_queue = Arc::new(RwLock::new(VecDeque::new()));
        let msg_queue_clone = msg_queue.clone();

        let message_handler: QbftSendFn =
            Box::new(move |message| msg_queue_clone.write().push_back(message));

        let qbft = Qbft::new(config, data, MessageId::from([0; 56]), message_handler);

        (SpecQbft(qbft), msg_queue)
    }

    // Create a new Signed SSV Message
    pub fn create_message(&self, message_type: QbftMessageType) -> SignedSSVMessage {
        let unsigned =
            self.0
                .new_unsigned_message_spec(message_type, Hash256::default(), vec![], vec![]);
        let signature = self.sign(unsigned.unsigned_message.clone());

        SignedSSVMessage::new(
            vec![signature],
            vec![OperatorId::from(1)], // todo!() do we pass this in??
            unsigned.unsigned_message.ssv_message,
            unsigned.unsigned_message.full_data,
        )
        .expect("Data is valid")
    }

    // In favor of not having to construct an entire NetworkMessageSender, just copy the signing
    // code
    fn sign(&self, unsigned: UnsignedSSVMessage) -> Vec<u8> {
        let _serialized = unsigned.ssv_message.as_ssz_bytes();
        // let mut signer = Signer::new(MessageDigest::sha256(), &self.private_key)?;
        // signer.update(&serialized)?;
        // signer.sign_to_vec()
        todo!()
    }

    // Confirm that merkle root of signed message equals the expected root
    pub fn verify_root(&self, msg: SignedSSVMessage, root: Hash256) -> bool {
        let spec_message: qbft_spec_types::SpecSignedSSVMessage = msg.into();
        spec_message.tree_hash_root() == root
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

// Grouping of merkalizable ssv_types
pub(crate) mod qbft_spec_types {
    use ssv_types::message::SSVMessage;
    use ssz_rs::prelude::*;
    use tree_hash_derive::TreeHash;
    use types::{
        typenum::{U13, U256},
        FixedVector, VariableList,
    };

    use super::SignedSSVMessage;

    #[derive(Clone, PartialEq, Eq, TreeHash)]
    pub struct SpecSignedSSVMessage {
        pub signatures: VariableList<FixedVector<u8, U256>, U13>,
        pub operator_ids: VariableList<u64, U13>,
        pub ssv_message: SpecSSVMessage,
        // pub full_data: VariableList<u8, U8388836>,
    }

    impl From<SignedSSVMessage> for SpecSignedSSVMessage {
        fn from(_signed_msg: SignedSSVMessage) -> Self {
            todo!()
        }
    }

    #[derive(Clone, PartialEq, Eq, TreeHash)]
    pub struct SpecSSVMessage {
        pub msg_type: u64,
        pub msg_id: [u8; 32],
        // pub data: VariableList<u8, typenum::U722412>,
    }

    impl From<SSVMessage> for SpecSSVMessage {
        fn from(_ssv_message: SSVMessage) -> Self {
            todo!()
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
        // .. now hash them
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        Ok(Hash256::from(hash))
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
