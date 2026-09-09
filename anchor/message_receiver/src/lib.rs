mod manager;

use libp2p::{
    PeerId,
    gossipsub::{Message, MessageId},
};
pub use message_validator::TopicContext;
use thiserror::Error;

pub use crate::{NetworkMessageReceiver, manager::*};

pub trait MessageReceiver {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
        topic_context: TopicContext,
    ) -> Result<(), Error>;
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] processor::Error),
}

#[cfg(test)]
mod proposer_view_tests;
