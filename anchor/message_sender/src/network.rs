use crate::MessageSender;
use database::{NetworkState, UniqueIndex};
use openssl::error::ErrorStack;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::sign::Signer;
use ssv_types::consensus::UnsignedSSVMessage;
use ssv_types::message::SignedSSVMessage;
use ssv_types::msgid::DutyExecutor;
use ssv_types::OperatorId;
use ssz::Encode;
use std::sync::Arc;
use subnet_tracker::SubnetId;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};

const SIGNER_NAME: &str = "message_sign_and_send";
const SENDER_NAME: &str = "message_send";

pub struct NetworkMessageSender {
    processor: processor::Senders,
    network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>,
    private_key: PKey<Private>,
    database: watch::Receiver<NetworkState>,
    operator_id: OperatorId,
    subnet_count: usize,
}

impl MessageSender for Arc<NetworkMessageSender> {
    fn sign_and_send(&self, message: UnsignedSSVMessage) {
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
                        Ok(signature) => signature,
                        Err(err) => {
                            error!(?err, "Creating signed message failed!");
                            return;
                        }
                    };
                    sender.do_send(message);
                },
                SIGNER_NAME,
            )
            .unwrap_or_else(|e| warn!("Failed to send to processor: {}", e));
    }

    fn send(&self, message: SignedSSVMessage) {
        let sender = self.clone();
        self.processor
            .urgent_consensus
            .send_blocking(
                move || {
                    sender.do_send(message);
                },
                SENDER_NAME,
            )
            .unwrap_or_else(|e| warn!("Failed to send to processor: {}", e));
    }
}

impl NetworkMessageSender {
    pub fn new(
        processor: processor::Senders,
        network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>,
        private_key: Rsa<Private>,
        database: watch::Receiver<NetworkState>,
        operator_id: OperatorId,
        subnet_count: usize,
    ) -> Result<Arc<Self>, String> {
        let private_key = PKey::from_rsa(private_key)
            .map_err(|err| format!("Failed to create PKey from RSA: {err}"))?;
        Ok(Arc::new(Self {
            processor,
            network_tx,
            private_key,
            database,
            operator_id,
            subnet_count,
        }))
    }

    fn do_send(&self, message: SignedSSVMessage) {
        let subnet = match self.determine_subnet(&message) {
            Ok(subnet) => subnet,
            Err(err) => {
                error!(?err, "Unable to determine subnet for outgoing message");
                return;
            }
        };
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

    fn determine_subnet(&self, message: &SignedSSVMessage) -> Result<SubnetId, String> {
        let msg_id = message.ssv_message().msg_id();
        let committee_id = match msg_id.duty_executor() {
            Some(DutyExecutor::Committee(committee_id)) => committee_id,
            Some(DutyExecutor::Validator(pubkey)) => {
                let database = self.database.borrow();
                let Some(metadata) = database.metadata().get_by(&pubkey) else {
                    return Err(format!("Unknown validator: {pubkey}"));
                };
                let Some(cluster) = database.clusters().get_by(&metadata.cluster_id) else {
                    return Err(format!(
                        "Inconsistent database, no cluster for validator: {pubkey}"
                    ));
                };
                cluster.committee_id()
            }
            None => return Err(format!("Invalid message id: {msg_id:?}",)),
        };
        Ok(SubnetId::from_committee(committee_id, self.subnet_count))
    }
}
