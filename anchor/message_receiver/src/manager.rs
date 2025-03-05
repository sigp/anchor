use crate::MessageReceiver;
use database::{NetworkState, UniqueIndex};
use message_validator::ValidatedSSVMessage;
use processor::Error;
use qbft_manager::QbftManager;
use signature_collector::SignatureCollectorManager;
use ssv_types::message::SignedSSVMessage;
use ssv_types::msgid::DutyExecutor;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::error;

const RECEIVER_NAME: &str = "message_receiver";

/// A message receiver that passes messages to responsible managers.
pub struct ManagerMessageReceiver {
    processor: processor::Senders,
    qbft_manager: Arc<QbftManager>,
    signature_collector: Arc<SignatureCollectorManager>,
    network_state_rx: watch::Receiver<NetworkState>,
}

impl MessageReceiver for Arc<ManagerMessageReceiver> {
    fn receive(
        &self,
        full_message: SignedSSVMessage,
        inner_message: ValidatedSSVMessage,
    ) -> Result<(), Error> {
        let receiver = self.clone();
        self.processor.urgent_consensus.send_blocking(move || {
            match full_message.ssv_message().msg_id().duty_executor() {
                Some(DutyExecutor::Validator(validator)) => {
                    if receiver
                        .network_state_rx
                        .borrow()
                        .shares()
                        .get_by(&validator)
                        .is_none()
                    {
                        // We are not a signer for this validator, return without passing.
                        return;
                    }
                }
                Some(DutyExecutor::Committee(committee)) => {
                    // TODO, this is very inefficient. Fix when aligning the database to cache what
                    // we actually need
                    let state = receiver.network_state_rx.borrow();
                    if !state.get_own_clusters().iter().any(|id| {
                        state
                            .clusters()
                            .get_by(id)
                            .map(|cluster| cluster.committee_id() == committee)
                            .unwrap_or(false)
                    }) {
                        // We are not a member for this committee, return without passing.
                        return;
                    }
                }
                None => {
                    error!(message_id = ?full_message.ssv_message().msg_id(), "Invalid message ID");
                }
            }

            match inner_message {
                ValidatedSSVMessage::QbftMessage(qbft_message) => {
                    if let Err(err) = receiver.qbft_manager.receive_data(full_message, qbft_message) {
                        error!(?err, "Unable to receive QBFT message");
                    }
                }
                ValidatedSSVMessage::PartialSignatureMessages(messages) => {
                    if let Err(err) = receiver.signature_collector.receive_partial_signatures(messages) {
                        error!(?err, "Unable to receive partial signature message");
                    }
                }
            }
        }, RECEIVER_NAME)
    }
}

impl ManagerMessageReceiver {
    pub fn new(
        processor: processor::Senders,
        qbft_manager: Arc<QbftManager>,
        signature_collector: Arc<SignatureCollectorManager>,
        network_state_rx: watch::Receiver<NetworkState>,
    ) -> Arc<Self> {
        Arc::new(Self {
            processor,
            qbft_manager,
            signature_collector,
            network_state_rx,
        })
    }
}
