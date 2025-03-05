mod manager;

#[cfg(feature = "testing")]
pub mod testing;

pub use crate::manager::*;

use message_validator::ValidatedSSVMessage;
use ssv_types::message::SignedSSVMessage;

pub trait MessageReceiver: Send + Sync {
    fn receive(
        &self,
        full_message: SignedSSVMessage,
        inner_message: ValidatedSSVMessage,
    ) -> Result<(), processor::Error>;
}
