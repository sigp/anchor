mod network;

#[cfg(feature = "testing")]
pub mod testing;

pub use crate::network::*;
use ssv_types::consensus::UnsignedSSVMessage;
use ssv_types::message::SignedSSVMessage;
use ssv_types::CommitteeId;

pub trait MessageSender: Send + Sync {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        committee_id: CommitteeId,
    ) -> Result<(), Error>;
    fn send(&self, message: SignedSSVMessage, committee_id: CommitteeId) -> Result<(), Error>;
}

#[derive(Debug)]
pub enum Error {
    Processor(processor::Error),
    NetworkQueueClosed,
}
