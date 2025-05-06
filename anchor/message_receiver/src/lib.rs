mod manager;

use gossipsub::{Message, MessageId};
use libp2p::PeerId;
use thiserror::Error;

pub use crate::{NetworkMessageReceiver, manager::*};

pub trait MessageReceiver {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
    ) -> Result<(), Error>;
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] processor::Error),
}
