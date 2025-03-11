use crate::MessageReceiver;
use database::{NetworkState, UniqueIndex};
use libp2p::gossipsub::{Message, MessageAcceptance, MessageId};
use libp2p::PeerId;
use message_validator::{ValidatedMessage, ValidatedSSVMessage, ValidatorService};
use processor::Error;
use qbft_manager::QbftManager;
use signature_collector::SignatureCollectorManager;
use ssv_types::msgid::DutyExecutor;
use std::sync::Arc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error};

const RECEIVER_NAME: &str = "message_receiver";

pub struct Outcome {
    pub message_id: MessageId,
    pub propagation_source: PeerId,
    pub action: MessageAcceptance,
}

/// A message receiver that passes messages to responsible managers.
pub struct ManagerMessageReceiver<V: ValidatorService + 'static> {
    processor: processor::Senders,
    qbft_manager: Arc<QbftManager>,
    signature_collector: Arc<SignatureCollectorManager>,
    network_state_rx: watch::Receiver<NetworkState>,
    outcome_tx: mpsc::Sender<Outcome>,
    validator: V,
}

impl<V: ValidatorService + 'static> MessageReceiver for Arc<ManagerMessageReceiver<V>> {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
    ) -> Result<(), Error> {
        let receiver = self.clone();
        self.processor.urgent_consensus.send_blocking(move || {
            let result = receiver.validator.validate(message.data);

            let action = match &result {
                Ok(_) => MessageAcceptance::Accept,
                Err(failure) => failure.into(),
            };

            if let Err(err) = receiver.outcome_tx.try_send(Outcome {
                message_id: message_id.clone(),
                propagation_source,
                action,
            }) {
                match err {
                    TrySendError::Closed(_) => {
                        error!("Validation result receiver dropped");
                    }
                    TrySendError::Full(_) => {
                        error!("Validation result receiver full");
                    }
                }
            }

            let ValidatedMessage {
                signed_ssv_message, ssv_message
            } = match result {
                Ok(message) => message,
                Err(failure) => {
                    debug!(?failure, msg = %message_id, "Validation failure");
                    return;
                }
            };

            match signed_ssv_message.ssv_message().msg_id().duty_executor() {
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
                    error!(message_id = ?signed_ssv_message.ssv_message().msg_id(), "Invalid message ID");
                }
            }

            match ssv_message {
                ValidatedSSVMessage::QbftMessage(qbft_message) => {
                    if let Err(err) = receiver.qbft_manager.receive_data(signed_ssv_message, qbft_message) {
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

impl<V: ValidatorService + 'static> ManagerMessageReceiver<V> {
    pub fn new(
        processor: processor::Senders,
        qbft_manager: Arc<QbftManager>,
        signature_collector: Arc<SignatureCollectorManager>,
        network_state_rx: watch::Receiver<NetworkState>,
        outcome_tx: mpsc::Sender<Outcome>,
        validator: V,
    ) -> Arc<Self> {
        Arc::new(Self {
            processor,
            qbft_manager,
            signature_collector,
            network_state_rx,
            outcome_tx,
            validator,
        })
    }
}
