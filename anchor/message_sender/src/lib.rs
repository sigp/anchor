mod network;

#[cfg(feature = "testing")]
pub mod testing;

pub use crate::network::*;
use ssv_types::consensus::UnsignedSSVMessage;
use ssv_types::message::SignedSSVMessage;

pub trait MessageSender: Send + Sync {
    fn sign_and_send(&self, message: UnsignedSSVMessage);
    fn send(&self, message: SignedSSVMessage);
}
