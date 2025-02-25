use crate::{Error, MessageSender};
use openssl::error::ErrorStack;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::sign::Signer;
use ssv_types::consensus::UnsignedSSVMessage;
use ssv_types::message::SignedSSVMessage;
use ssv_types::{CommitteeId, OperatorId};
use ssz::Encode;
use std::sync::Arc;
use subnet_tracker::SubnetId;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::{debug, error, warn};

const SIGNER_NAME: &str = "message_sign_and_send";
const SENDER_NAME: &str = "message_send";

pub struct NetworkMessageSender {
    processor: processor::Senders,
    network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>,
    private_key: PKey<Private>,
    operator_id: OperatorId,
    subnet_count: usize,
}

impl MessageSender for Arc<NetworkMessageSender> {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        committee_id: CommitteeId,
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

impl NetworkMessageSender {
    pub fn new(
        processor: processor::Senders,
        network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>,
        private_key: Rsa<Private>,
        operator_id: OperatorId,
        subnet_count: usize,
    ) -> Result<Arc<Self>, String> {
        let private_key = PKey::from_rsa(private_key)
            .map_err(|err| format!("Failed to create PKey from RSA: {err}"))?;
        Ok(Arc::new(Self {
            processor,
            network_tx,
            private_key,
            operator_id,
            subnet_count,
        }))
    }

    fn do_send(&self, message: SignedSSVMessage, committee_id: CommitteeId) {
        let subnet = SubnetId::from_committee(committee_id, self.subnet_count);
        match self.network_tx.try_send((subnet, message.as_ssz_bytes())) {
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
