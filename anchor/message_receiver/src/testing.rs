use crate::MessageReceiver;
use ssv_types::message::SignedSSVMessage;
use tracing::debug;
use message_validator::ValidatedSSVMessage;
use processor::Error;

pub struct MessageReceiverMock;

impl MessageReceiver for MessageReceiverMock {
    fn receive(&self, full_message: SignedSSVMessage, inner_message: ValidatedSSVMessage) -> Result<(), Error> {
        debug!(?full_message, ?inner_message, "received message");
        Ok(())
    }
}