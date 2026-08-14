use std::sync::Arc;

use database::OwnOperatorId;
use message_validator::{DutiesProvider, MessageAcceptance, TopicContext, Validator};
use openssl::{
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Rsa,
    sign::Signer,
};
use slot_clock::SlotClock;
use ssv_types::{
    CommitteeId, RSA_SIGNATURE_SIZE, consensus::UnsignedSSVMessage, message::SignedSSVMessage,
};
use ssz::Encode;
use subnet_service::SubnetService;
use tokio::sync::{mpsc, mpsc::error::TrySendError, watch};
use tracing::{debug, error, trace, warn};

use crate::{Error, MessageCallback, MessageSender, SigningError};

const SIGNER_NAME: &str = "message_sign_and_send";
const SENDER_NAME: &str = "message_send";

/// Configuration for creating a NetworkMessageSender
pub struct NetworkMessageSenderConfig<S: SlotClock, D: DutiesProvider> {
    pub processor: processor::Senders,
    /// Channel to send messages to the network. Tuple of (topic string, message bytes).
    /// Per SIP-43, the topic is determined by the message's slot.
    pub network_tx: mpsc::Sender<(String, Vec<u8>)>,
    pub private_key: Rsa<Private>,
    pub operator_id: OwnOperatorId,
    pub validator: Option<Arc<Validator<S, D>>>,
    pub is_synced: watch::Receiver<bool>,
    pub subnet_service: Arc<SubnetService<S>>,
}

pub struct NetworkMessageSender<S: SlotClock, D: DutiesProvider> {
    processor: processor::Senders,
    /// Channel to send messages to the network. Tuple of (topic string, message bytes).
    network_tx: mpsc::Sender<(String, Vec<u8>)>,
    private_key: PKey<Private>,
    operator_id: OwnOperatorId,
    validator: Option<Arc<Validator<S, D>>>,
    is_synced: watch::Receiver<bool>,
    subnet_service: Arc<SubnetService<S>>,
}

impl<S: SlotClock + 'static, D: DutiesProvider> MessageSender for Arc<NetworkMessageSender<S, D>> {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        committee_id: CommitteeId,
        additional_message_callback: Option<Box<MessageCallback>>,
    ) -> Result<(), Error> {
        if self.network_tx.is_closed() {
            return Err(Error::NetworkQueueClosed);
        }
        let Some(operator_id) = self.operator_id.get() else {
            return Err(Error::OwnOperatorIdUnknown);
        };
        if !*self.is_synced.borrow() {
            return Err(Error::NotSynced);
        }

        let sender = self.clone();
        self.processor
            .urgent_consensus
            .send_blocking(
                move || {
                    let signature = match sender.sign(&message) {
                        Ok(signature) => signature,
                        Err(err) => {
                            error!(?err, "Signing message failed!");
                            return;
                        }
                    };
                    let message = match SignedSSVMessage::new(
                        vec![signature],
                        vec![operator_id],
                        message.ssv_message,
                        message.full_data,
                    ) {
                        Ok(signed_message) => signed_message,
                        Err(err) => {
                            error!(?err, "Creating signed message failed!");
                            return;
                        }
                    };
                    if let Some(callback) = additional_message_callback {
                        callback(&message);
                    }
                    sender.do_send(message, committee_id);
                },
                SIGNER_NAME,
            )
            .map_err(Error::Processor)
    }

    fn send(&self, message: SignedSSVMessage, committee_id: CommitteeId) -> Result<(), Error> {
        if self.network_tx.is_closed() {
            return Err(Error::NetworkQueueClosed);
        }
        if !*self.is_synced.borrow() {
            return Err(Error::NotSynced);
        }

        let sender = self.clone();
        self.processor
            .urgent_consensus
            .send_blocking(
                move || {
                    sender.do_send(message, committee_id);
                },
                SENDER_NAME,
            )
            .map_err(Error::Processor)
    }
}

impl<S: SlotClock + 'static, D: DutiesProvider> NetworkMessageSender<S, D> {
    pub fn new(config: NetworkMessageSenderConfig<S, D>) -> Result<Arc<Self>, String> {
        let private_key = PKey::from_rsa(config.private_key)
            .map_err(|err| format!("Failed to create PKey from RSA: {err}"))?;
        Ok(Arc::new(Self {
            processor: config.processor,
            network_tx: config.network_tx,
            private_key,
            operator_id: config.operator_id,
            validator: config.validator,
            is_synced: config.is_synced,
            subnet_service: config.subnet_service,
        }))
    }

    fn do_send(&self, message: SignedSSVMessage, committee_id: CommitteeId) {
        let message_bytes = message.as_ssz_bytes();

        // For outgoing messages, we use default TopicContext (no topic validation)
        // since we're just doing a sanity check on our own message content
        if let Some(validator) = self.validator.as_ref()
            && let Err(err) = validator
                .validate(&message_bytes, &TopicContext::default(), None)
                .as_result()
        {
            // `Reject` is more severe and can be punished by other peers. We should not have
            // created this message ever, while `Ignore` can be triggered simply because the message
            // is irrelevant by now.
            if let MessageAcceptance::Reject = MessageAcceptance::from(err) {
                warn!(?err, "Validation of outgoing message failed (Reject)");
                debug!(msg = %message, "Failing message");
            } else {
                debug!(?err, "Validation of outgoing message failed (Ignore)");
            }
            return;
        }

        // Extract slot from message for slot-based topic routing (per SIP-43)
        let message_slot = match message.ssv_message().extract_slot() {
            Some(slot) => slot,
            None => {
                warn!(
                    ?committee_id,
                    "Cannot extract slot from message for topic routing"
                );
                return;
            }
        };

        // Use subnet service for slot-based subnet calculation
        let subnet = match self
            .subnet_service
            .subnet_for_committee_at_slot(committee_id, message_slot)
        {
            Ok(subnet) => subnet,
            Err(e) => {
                warn!(?committee_id, ?e, "Cannot calculate subnet for message");
                return;
            }
        };

        // Create topic based on message slot (per SIP-43 slot-based routing)
        let topic = self
            .subnet_service
            .router()
            .topic_for_subnet_at_slot(subnet, message_slot);

        match self.network_tx.try_send((topic.clone(), message_bytes)) {
            Ok(_) => trace!(?subnet, %topic, "Successfully sent message to network"),
            Err(TrySendError::Closed(_)) => warn!("Network queue closed (shutting down?)"),
            Err(TrySendError::Full(_)) => warn!("Network queue full, unable to send message!"),
        }
    }

    fn sign(&self, message: &UnsignedSSVMessage) -> Result<[u8; RSA_SIGNATURE_SIZE], SigningError> {
        let serialized = message.ssv_message.as_ssz_bytes();
        let mut signer = Signer::new(MessageDigest::sha256(), &self.private_key)?;
        signer.update(&serialized)?;
        let mut signature = [0u8; RSA_SIGNATURE_SIZE];
        let len = signer.sign(&mut signature)?;
        if len != RSA_SIGNATURE_SIZE {
            return Err(SigningError::IncorrectCiphertextLength(len));
        }
        Ok(signature)
    }
}
