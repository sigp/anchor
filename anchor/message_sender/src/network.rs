use std::sync::Arc;

use message_validator::{DutiesProvider, MessageAcceptance, Validator};
use openssl::{
    error::ErrorStack,
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Rsa,
    sign::Signer,
};
use slot_clock::SlotClock;
use ssv_types::{
    CommitteeId, OperatorId, consensus::UnsignedSSVMessage, message::SignedSSVMessage,
};
use ssz::Encode;
use subnet_tracker::SubnetId;
use tokio::sync::{mpsc, mpsc::error::TrySendError};
use tracing::{debug, error, warn};

use crate::{Error, MessageCallback, MessageSender};

const SIGNER_NAME: &str = "message_sign_and_send";
const SENDER_NAME: &str = "message_send";

pub struct NetworkMessageSender<S: SlotClock, D: DutiesProvider> {
    processor: processor::Senders,
    network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>,
    private_key: PKey<Private>,
    operator_id: OperatorId,
    validator: Option<Arc<Validator<S, D>>>,
    subnet_count: usize,
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
                        vec![sender.operator_id],
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

impl<S: SlotClock, D: DutiesProvider> NetworkMessageSender<S, D> {
    pub fn new(
        processor: processor::Senders,
        network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>,
        private_key: Rsa<Private>,
        operator_id: OperatorId,
        validator: Option<Arc<Validator<S, D>>>,
        subnet_count: usize,
    ) -> Result<Arc<Self>, String> {
        let private_key = PKey::from_rsa(private_key)
            .map_err(|err| format!("Failed to create PKey from RSA: {err}"))?;
        Ok(Arc::new(Self {
            processor,
            network_tx,
            private_key,
            operator_id,
            validator,
            subnet_count,
        }))
    }

    fn do_send(&self, message: SignedSSVMessage, committee_id: CommitteeId) {
        let message_bytes = message.as_ssz_bytes();

        if let Some(validator) = self.validator.as_ref() {
            if let Err(err) = validator.validate(&message_bytes).as_result() {
                // `Reject` is more severe and can be punished by other peers. We should not have
                // created this message ever, while `Ignore` can be triggered simply because the
                // message is irrelevant by now.
                if let MessageAcceptance::Reject = MessageAcceptance::from(err) {
                    warn!(?err, "Validation of outgoing message failed (Reject)");
                    debug!(msg = %message, "Failing message");
                } else {
                    debug!(?err, "Validation of outgoing message failed (Ignore)");
                }
                return;
            }
        }

        let subnet = SubnetId::from_committee(committee_id, self.subnet_count);
        match self.network_tx.try_send((subnet, message_bytes)) {
            Ok(_) => debug!(?subnet, "Successfully sent message to network"),
            Err(TrySendError::Closed(_)) => warn!("Network queue closed (shutting down?)"),
            Err(TrySendError::Full(_)) => warn!("Network queue full, unable to send message!"),
        }
    }

    fn sign(&self, message: &UnsignedSSVMessage) -> Result<Vec<u8>, ErrorStack> {
        let serialized = message.ssv_message.as_ssz_bytes();
        let mut signer = Signer::new(MessageDigest::sha256(), &self.private_key)?;
        signer.update(&serialized)?;
        signer.sign_to_vec()
    }
}
