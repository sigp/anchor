mod manager;

use gossipsub::{Message, MessageId, TopicHash};
use libp2p::PeerId;
use thiserror::Error;

pub use crate::{NetworkMessageReceiver, manager::*};

pub trait MessageReceiver {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
        topic: TopicHash,
    ) -> Result<(), Error>;
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] processor::Error),
}
