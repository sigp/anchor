use crate::MessageSender;
use ssv_types::consensus::UnsignedSSVMessage;
use ssv_types::message::SignedSSVMessage;
use ssv_types::OperatorId;
use tokio::sync::mpsc;

pub struct TestingMessageSender {
    message_tx: mpsc::UnboundedSender<SignedSSVMessage>,
    operator_id: OperatorId,
}

impl MessageSender for TestingMessageSender {
    fn sign_and_send(&self, message: UnsignedSSVMessage) {
        let message = SignedSSVMessage::new(
            vec![vec![]],
            vec![self.operator_id],
            message.ssv_message,
            message.full_data,
        )
        .unwrap();
        self.send(message);
    }

    fn send(&self, message: SignedSSVMessage) {
        self.message_tx.send(message).unwrap();
    }
}

impl TestingMessageSender {
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
