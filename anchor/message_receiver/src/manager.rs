use std::sync::Arc;

use database::{NetworkState, NonUniqueIndex, UniqueIndex};
use gossipsub::{Message, MessageAcceptance, MessageId};
use libp2p::PeerId;
use message_validator::{DutiesProvider, ValidatedMessage, ValidatedSSVMessage, Validator};
use qbft_manager::QbftManager;
use signature_collector::SignatureCollectorManager;
use slot_clock::SlotClock;
use ssv_types::msgid::DutyExecutor;
use tokio::sync::{mpsc, mpsc::error::TrySendError, watch};
use tracing::{debug, debug_span, error, trace};

use crate::MessageReceiver;

const RECEIVER_NAME: &str = "message_receiver";

pub struct Outcome {
    pub message_id: MessageId,
    pub propagation_source: PeerId,
    pub action: MessageAcceptance,
}

/// A message receiver that passes messages to responsible managers.
pub struct NetworkMessageReceiver<S: SlotClock, D: DutiesProvider> {
    processor: processor::Senders,
    qbft_manager: Arc<QbftManager>,
    signature_collector: Arc<SignatureCollectorManager>,
    network_state_rx: watch::Receiver<NetworkState>,
    outcome_tx: mpsc::Sender<Outcome>,
    validator: Arc<Validator<S, D>>,
}

impl<S: SlotClock + 'static, D: DutiesProvider> NetworkMessageReceiver<S, D> {
    pub fn new(
        processor: processor::Senders,
        qbft_manager: Arc<QbftManager>,
        signature_collector: Arc<SignatureCollectorManager>,
        network_state_rx: watch::Receiver<NetworkState>,
        outcome_tx: mpsc::Sender<Outcome>,
        validator: Arc<Validator<S, D>>,
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

impl<S: SlotClock + 'static, D: DutiesProvider> MessageReceiver
    for Arc<NetworkMessageReceiver<S, D>>
{
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
    ) -> Result<(), crate::Error> {
        let receiver = self.clone();
        self.processor.urgent_consensus.send_blocking(
            move || {
                let span = debug_span!("message_receiver", msg=%message_id);
                let _enter = span.enter();

                let result = receiver.validator.validate(&message.data);

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
                    signed_ssv_message,
                    ssv_message,
                } = match result {
                    Ok(message) => message,
                    Err(failure) => {
                        debug!(gosspisub_message_id = ?message_id, ?failure, "Validation failure");
                        return;
                    }
                };

                let msg_id = signed_ssv_message.ssv_message().msg_id().clone();

                match msg_id.duty_executor() {
                    Some(DutyExecutor::Validator(validator)) => {
                        if receiver
                            .network_state_rx
                            .borrow()
                            .shares()
                            .get_by(&validator)
                            .is_none()
                        {
                            // We are not a signer for this validator, return without passing.
                            trace!(gosspisub_message_id = ?message_id, ssv_msg_id = ?msg_id, ?validator, "Not interested");
                            return;
                        }
                    }
                    Some(DutyExecutor::Committee(committee_id)) => {
                        let state = receiver.network_state_rx.borrow();
                        let Some(own_id) = state.get_own_id() else {
                            // We do not know who we are yet.
                            return;
                        };

                        let committee = state.clusters().get_all_by(&committee_id);
                        // We only need to check one cluster, as all clusters will have the same set
                        // of operators.
                        let is_member = committee.clone()
                            .and_then(|mut v| v.pop())
                            .map(|c| c.cluster_members.contains(&own_id))
                            .unwrap_or(false);

                        if !is_member {
                            // We are not a member for this committee, return without passing.
                            trace!(gossipsub_message_id = ?message_id, ssv_msg_id = ?msg_id, ?committee, "Not interested");
                            return;
                        }
                    }
                    None => {
                        error!(gossipsub_message_id = ?message_id, ssv_msg_id = ?msg_id, "Invalid message ID");
                        return;
                    }
                }

                match ssv_message {
                    ValidatedSSVMessage::QbftMessage(qbft_message) => {
                        if let Err(err) = receiver
                            .qbft_manager
                            .receive_data(signed_ssv_message, qbft_message)
                        {
                            error!(gossipsub_message_id = ?message_id, ssv_msg_id = ?msg_id, ?err, "Unable to receive QBFT message");
                        }
                    }
                    ValidatedSSVMessage::PartialSignatureMessages(messages) => {
                        if let Err(err) = receiver
                            .signature_collector
                            .receive_partial_signatures(messages)
                        {
                            error!(gossipsub_message_id = ?message_id, ssv_msg_id = ?msg_id, ?err, "Unable to receive partial signature message");
                        }
                    }
                }
            },
            RECEIVER_NAME,
        )?;
        Ok(())
    }
}
