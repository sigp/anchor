mod network;

#[cfg(feature = "testing")]
pub mod testing;

pub use crate::network::*;
use ssv_types::consensus::UnsignedSSVMessage;
use ssv_types::message::SignedSSVMessage;
use tokio::sync::mpsc::error::TrySendError;

pub trait MessageSender: Send + Sync {
    fn sign_and_send(&self, message: UnsignedSSVMessage) -> Result<(), Error>;
    fn send(&self, message: SignedSSVMessage) -> Result<(), Error>;
}

#[derive(Debug)]
pub enum Error {
    Processor(TrySendError<processor::WorkItem>),
    NetworkQueueClosed,
}
