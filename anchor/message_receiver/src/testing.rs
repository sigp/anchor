use crate::MessageReceiver;
use libp2p::gossipsub::{Message, MessageId};
use libp2p::PeerId;
use message_validator::ValidatorService;
use processor::Error;
use tracing::debug;

pub struct MessageReceiverMock<V: ValidatorService + 'static> {
    /// Only used for logging. Useful for testing
    name: String,
    validator: V,
}

impl<V: ValidatorService + 'static> MessageReceiver for MessageReceiverMock<V> {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
    ) -> Result<(), Error> {
        debug!(
            ?propagation_source,
            ?message_id,
            ?message,
            receiver = self.name,
            "Received message"
        );
        match self
            .validator
            .validate(message_id.clone(), propagation_source, message.data)
        {
            None => {
                debug!(?message_id, receiver = self.name, "Validation failed");
            }
            Some(message) => {
                debug!(
                    ?message_id,
                    ?message,
                    receiver = self.name,
                    "Validation succeeded"
                );
            }
        }
        Ok(())
    }
}

impl<V: ValidatorService + 'static> MessageReceiverMock<V> {
    pub fn new(name: String, validator: V) -> Self {
        Self { name, validator }
    }
}
