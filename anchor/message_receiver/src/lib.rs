mod manager;

#[cfg(feature = "testing")]
pub mod testing;

pub use crate::manager::*;
use gossipsub::{Message, MessageId};
use libp2p::PeerId;

pub trait MessageReceiver: Send + Sync {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
    ) -> Result<(), processor::Error>;
}
