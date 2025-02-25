use crate::{Error, MessageSender};
use ssv_types::consensus::UnsignedSSVMessage;
use ssv_types::message::SignedSSVMessage;
use ssv_types::{CommitteeId, OperatorId};
use tokio::sync::mpsc;

pub struct MockMessageSender {
    message_tx: mpsc::UnboundedSender<SignedSSVMessage>,
    operator_id: OperatorId,
}

impl MessageSender for MockMessageSender {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        committee_id: CommitteeId,
    ) -> Result<(), Error> {
        let message = SignedSSVMessage::new(
            vec![vec![]],
            vec![self.operator_id],
            message.ssv_message,
            message.full_data,
        )
        .unwrap();
        self.send(message, committee_id)
    }

    fn send(&self, message: SignedSSVMessage, _committee_id: CommitteeId) -> Result<(), Error> {
        self.message_tx
            .send(message)
            .map_err(|_| Error::NetworkQueueClosed)
    }
}

impl MockMessageSender {
    pub fn new(
        message_tx: mpsc::UnboundedSender<SignedSSVMessage>,
        operator_id: OperatorId,
    ) -> Self {
        Self {
            message_tx,
            operator_id,
        }
    }
}
