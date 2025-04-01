mod network;

#[cfg(feature = "testing")]
pub mod testing;

use ssv_types::{consensus::UnsignedSSVMessage, message::SignedSSVMessage, CommitteeId};

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
}
