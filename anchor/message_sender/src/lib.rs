mod network;

pub mod impostor;
#[cfg(feature = "testing")]
pub mod testing;

use ssv_types::{CommitteeId, consensus::UnsignedSSVMessage, message::SignedSSVMessage};

pub use crate::network::*;

type MessageCallback = dyn FnOnce(&SignedSSVMessage) + Send + 'static;

pub trait MessageSender: Send + Sync {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        committee_id: CommitteeId,
        additional_message_callback: Option<Box<MessageCallback>>,
    ) -> Result<(), Error>;
    fn send(&self, message: SignedSSVMessage, committee_id: CommitteeId) -> Result<(), Error>;
}

#[derive(Debug)]
pub enum Error {
    Processor(processor::Error),
    NetworkQueueClosed,
    OwnOperatorIdUnknown,
    NotSynced,
}
