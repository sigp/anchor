mod manager;

pub use crate::manager::*;
use libp2p::gossipsub::{Message, MessageId};
use libp2p::PeerId;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] processor::Error),
}

pub trait MessageReceiver: Send + Sync {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
    ) -> Result<(), crate::Error>;
}
