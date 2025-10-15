use std::sync::{Arc, Mutex};

use database::{NetworkState, NonUniqueIndex, UniqueIndex};
use gossipsub::{Message, MessageAcceptance, MessageId};
use libp2p::PeerId;
use message_validator::{
    DutiesProvider, ValidatedMessage, ValidatedSSVMessage, ValidationResult, Validator,
};
use qbft_manager::QbftManager;
use signature_collector::SignatureCollectorManager;
use slot_clock::SlotClock;
use ssv_types::msgid::DutyExecutor;
use tokio::sync::{mpsc, mpsc::error::TrySendError, oneshot, watch};
use tracing::{debug, debug_span, error, trace};

use crate::MessageReceiver;

/// Callback to check if a message indicates a doppelgänger (returns true if twin detected)
pub type DoppelgangerChecker = Box<
    dyn Fn(&ssv_types::message::SignedSSVMessage, &ssv_types::consensus::QbftMessage) -> bool
        + Send
        + Sync,
>;

/// Configuration for operator doppelgänger detection
pub struct DoppelgangerConfig {
    pub checker: Arc<DoppelgangerChecker>,
    pub shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

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
    is_synced: watch::Receiver<bool>,
    outcome_tx: mpsc::Sender<Outcome>,
    validator: Arc<Validator<S, D>>,
    doppelganger_config: Option<DoppelgangerConfig>,
}

impl<S: SlotClock + 'static, D: DutiesProvider> NetworkMessageReceiver<S, D> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        processor: processor::Senders,
        qbft_manager: Arc<QbftManager>,
        signature_collector: Arc<SignatureCollectorManager>,
        network_state_rx: watch::Receiver<NetworkState>,
        is_synced: watch::Receiver<bool>,
        outcome_tx: mpsc::Sender<Outcome>,
        validator: Arc<Validator<S, D>>,
        doppelganger_config: Option<DoppelgangerConfig>,
    ) -> Arc<Self> {
        Arc::new(Self {
            processor,
            qbft_manager,
            signature_collector,
            network_state_rx,
            is_synced,
            outcome_tx,
            validator,
            doppelganger_config,
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

                let mut action = MessageAcceptance::from(&result);

                // If we are not synced, do not punish peers to avoid banning peers during
                // historical sync.
                if let MessageAcceptance::Reject = action && !*receiver.is_synced.borrow() {
                    action = MessageAcceptance::Ignore;
                }

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
                    ValidationResult::Success(message) => message,
                    ValidationResult::PreDecodeFailure(failure) => {
                        debug!(
                            msg = hex::encode(&message.data),
                            ?failure,
                            "Validation failure"
                        );
                        return
                    }
                    ValidationResult::PostDecodeFailure(failure, msg) => {
                        debug!(%msg, ?failure, "Validation failure");
                        return
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

                        // We only need to check one cluster, as all clusters will have the same set
                        // of operators.
                        let is_member = state
                            .clusters()
                            .get_all_by(&committee_id)
                            .next()
                            .map(|c| c.cluster_members.contains(&own_id))
                            .unwrap_or(false);

                        if !is_member {
                            // We are not a member for this committee, return without passing.
                            trace!(gossipsub_message_id = ?message_id, ssv_msg_id = ?msg_id, ?committee_id, "Not interested");
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
                        // Check for operator doppelgänger before processing
                        if let Some(config) = &receiver.doppelganger_config
                            && (config.checker)(&signed_ssv_message, &qbft_message)
                        {
                            error!(
                                gossipsub_message_id = ?message_id,
                                ssv_msg_id = ?msg_id,
                                "Operator doppelgänger detected! Triggering shutdown."
                            );

                            // Trigger shutdown - we'll only do this once
                            if let Ok(mut guard) = config.shutdown_tx.lock()
                                && let Some(tx) = guard.take()
                            {
                                let _ = tx.send(());
                            }

                            return;
                        }

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
