mod manager;

use libp2p::{
    PeerId,
    gossipsub::{Message, MessageId},
};
pub use message_validator::ParsedTopic;
use thiserror::Error;

pub use crate::{NetworkMessageReceiver, manager::*};

pub trait MessageReceiver {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
        parsed_topic: ParsedTopic,
    ) -> Result<(), Error>;
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] processor::Error),
}
