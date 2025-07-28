mod network;

pub mod impostor;
#[cfg(feature = "testing")]
pub mod testing;

use openssl::error::ErrorStack;
use ssv_types::{CommitteeId, consensus::UnsignedSSVMessage, message::SignedSSVMessage};
use thiserror::Error as ThisError;

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

#[derive(Debug, ThisError)]
enum SigningError {
    #[error("Signing error: {0}")]
    SignerError(#[from] ErrorStack),
    #[error("Ciphertext has {0} bytes, expected 256")]
    IncorrectCiphertextLength(usize),
}
