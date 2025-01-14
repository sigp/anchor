use dashmap::DashMap;
use processor::{DropOnFinish, Senders, WorkItem};
use ssv_types::{ClusterId, OperatorId};
use std::collections::{hash_map, HashMap};
use std::mem;
use std::sync::Arc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::error::RecvError;
use tokio::sync::{mpsc, oneshot};
use tracing::error;
use types::{Hash256, SecretKey, Signature};

const COLLECTOR_NAME: &str = "signature_collector";
const COLLECTOR_MESSAGE_NAME: &str = "signature_collector_message";
const SIGNER_NAME: &str = "signer";

pub struct SignatureCollectorManager {
    processor: Senders,
    signature_collectors: DashMap<Hash256, UnboundedSender<CollectorMessage>>,
}

impl SignatureCollectorManager {
    pub fn new(processor: Senders) -> Self {
        Self {
            processor,
            signature_collectors: DashMap::new(),
        }
    }

    pub async fn sign_and_collect(
        self: &Arc<Self>,
        request: SignatureRequest,
        our_operator_id: OperatorId,
        our_key: SecretKey,
    ) -> Result<Arc<Signature>, CollectionError> {
        let (result_tx, result_rx) = oneshot::channel();

        // first, register notifier
        let cloned_request = request.clone();
        let manager = self.clone();
        self.processor.permitless.send_immediate(
            move |drop_on_finish| {
                let sender = manager.get_or_spawn(cloned_request);
                let _ = sender.send(CollectorMessage {
                    kind: CollectorMessageKind::Notify { notify: result_tx },
                    drop_on_finish,
                });
            },
            COLLECTOR_MESSAGE_NAME,
        )?;

        // then, trigger signing via blocking code
        let manager = self.clone();
        self.processor.urgent_consensus.send_blocking(
            move || {
                let signature = Box::new(our_key.sign(request.signing_root));
                let _ = manager.receive_partial_signature(request, our_operator_id, signature);
                // TODO send signature over network
            },
            SIGNER_NAME,
        )?;

        // finally, we resolve the collector future - if we are lucky, the signature is even already
        // done (as we received enough shares before this fn is even called)
        Ok(result_rx.await?)
    }

    pub fn receive_partial_signature(
        self: &Arc<Self>,
        request: SignatureRequest,
        operator_id: OperatorId,
        signature: Box<Signature>,
    ) -> Result<(), CollectionError> {
        let manager = self.clone();
        self.processor.permitless.send_immediate(
            move |drop_on_finish| {
                let sender = manager.get_or_spawn(request);
                let _ = sender.send(CollectorMessage {
                    kind: CollectorMessageKind::PartialSignature {
                        operator_id,
                        signature,
                    },
                    drop_on_finish,
                });
            },
            COLLECTOR_MESSAGE_NAME,
        )?;
        Ok(())
    }

    pub fn remove(&self, signing_hash: Hash256) {
        self.signature_collectors.remove(&signing_hash);
    }

    fn get_or_spawn(&self, request: SignatureRequest) -> UnboundedSender<CollectorMessage> {
        match self.signature_collectors.entry(request.signing_root) {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                let (tx, rx) = mpsc::unbounded_channel();
                let tx = entry.insert(tx);
                let _ = self
                    .processor
                    .permitless
                    .send_async(Box::pin(signature_collector(rx, request)), COLLECTOR_NAME);
                tx.clone()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SignatureRequest {
    pub cluster_id: ClusterId,
    pub signing_root: Hash256,
    pub threshold: u64,
}

pub struct CollectorMessage {
    pub kind: CollectorMessageKind,
    pub drop_on_finish: DropOnFinish,
}

pub enum CollectorMessageKind {
    Notify {
        notify: oneshot::Sender<Arc<Signature>>,
    },
    PartialSignature {
        operator_id: OperatorId,
        signature: Box<Signature>,
    },
}

#[derive(Debug, Clone)]
pub enum CollectionError {
    QueueClosedError,
    QueueFullError,
    CollectionTimeout,
}

impl From<TrySendError<WorkItem>> for CollectionError {
    fn from(value: TrySendError<WorkItem>) -> Self {
        match value {
            TrySendError::Full(_) => CollectionError::QueueFullError,
            TrySendError::Closed(_) => CollectionError::QueueClosedError,
        }
    }
}

impl From<RecvError> for CollectionError {
    fn from(_: RecvError) -> Self {
        CollectionError::QueueClosedError
    }
}

async fn signature_collector(
    mut rx: mpsc::UnboundedReceiver<CollectorMessage>,
    request: SignatureRequest,
) {
    let mut notifiers = vec![];
    let mut signature_share = HashMap::new();
    let mut full_signature: Option<Arc<Signature>> = None;

    while let Some(message) = rx.recv().await {
        match message.kind {
            CollectorMessageKind::Notify { notify } => {
                if let Some(full_signature) = &full_signature {
                    let _ = notify.send(full_signature.clone());
                } else {
                    notifiers.push(notify);
                }
            }
            CollectorMessageKind::PartialSignature {
                operator_id,
                signature,
            } => {
                if full_signature.is_some() {
                    // already got the full signature :)
                    continue;
                }

                // always insert to make sure we're not duplicated
                match signature_share.entry(operator_id) {
                    hash_map::Entry::Vacant(entry) => {
                        entry.insert(signature);
                    }
                    hash_map::Entry::Occupied(entry) => {
                        if entry.get() != &signature {
                            error!(
                                ?operator_id,
                                "received conflicting signatures from operator"
                            );
                        }
                    }
                }

                if signature_share.len() as u64 >= request.threshold {
                    // TODO move to blocking threadpool?

                    // TODO do magic crypto to actually restore signature, for now hackily take first one
                    let signature = mem::take(&mut signature_share)
                        .into_iter()
                        .next()
                        .unwrap()
                        .1
                        .into();

                    for notifier in mem::take(&mut notifiers) {
                        let _ = notifier.send(Arc::clone(&signature));
                    }
                    full_signature = Some(signature);
                }
            }
        }
    }
}
