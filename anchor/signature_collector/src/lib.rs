use std::{
    collections::{HashMap, hash_map},
    mem,
    sync::Arc,
};

use bls_lagrange::KeyId;
use dashmap::{DashMap, Entry};
use database::OwnOperatorId;
use message_sender::MessageSender;
use processor::{Error, Error::Queue, Senders, work::DropOnFinish};
use slot_clock::SlotClock;
pub use ssv_types::{
    CommitteeId, OperatorId, ValidatorIndex,
    consensus::UnsignedSSVMessage,
    domain_type::DomainType,
    message::{MsgType, SSVMessage},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
};
use ssz::Encode;
use tokio::{
    sync::{
        mpsc,
        mpsc::{UnboundedSender, error::TrySendError},
        oneshot,
        oneshot::error::RecvError,
    },
    time::sleep,
};
use tracing::{Instrument, debug, debug_span, error, trace, warn};
use types::{Hash256, PublicKeyBytes, SecretKey, Signature, Slot};

const COLLECTOR_NAME: &str = "signature_collector";
const COLLECTOR_MESSAGE_NAME: &str = "signature_collector_message";
const COLLECTOR_CLEANER_NAME: &str = "signature_collector_cleaner";
const SIGNER_NAME: &str = "partial_signer";

/// number of slots to keep before the current slot
const SIGNATURE_COLLECTOR_RETAIN_SLOTS: u64 = 1;

/// A handle to message the instance collecting a single specific signature
struct SignatureCollector {
    sender: UnboundedSender<CollectorMessage>,
    for_slot: Slot,
}

/// Outgoing partial signature messages that collected for a committee
/// As soon as the partial signature for every validator in the committee is ready, it is sent.
struct CommitteeSignatures {
    collected_signatures: Vec<PartialSignatureMessage>,
    for_slot: Slot,
}

pub struct SignatureCollectorManager {
    /// The handle to the processor, for queueing messages to the instances.
    processor: Senders,
    /// The local operator we act for.
    operator_id: OwnOperatorId,
    /// The network domain to be embedded in the message id of outgoing messages.
    domain: DomainType,
    /// A message sender used for outgoing messages.
    message_sender: Arc<dyn MessageSender>,
    /// A map from the signing root and signing validator to the corresponding signature collector.
    signature_collectors: DashMap<(Hash256, ValidatorIndex), SignatureCollector>,
    /// A map from a hash of an underlying decided committee value and committee id to a container
    /// for all partial signatures based on that value for the committee.
    /// Note that this hash may differ from the actual signing root.
    committee_signatures: DashMap<(Hash256, CommitteeId), CommitteeSignatures>,
}

impl SignatureCollectorManager {
    pub fn new(
        processor: Senders,
        operator_id: OwnOperatorId,
        domain: DomainType,
        message_sender: Arc<dyn MessageSender>,
        slot_clock: impl SlotClock + 'static,
    ) -> Result<Arc<Self>, CollectionError> {
        let manager = Arc::new(Self {
            processor,
            operator_id,
            domain,
            message_sender,
            signature_collectors: DashMap::new(),
            committee_signatures: DashMap::new(),
        });

        manager.processor.permitless.send_async(
            Arc::clone(&manager).cleaner(slot_clock),
            COLLECTOR_CLEANER_NAME,
        )?;

        Ok(manager)
    }

    /// Sign a message and wait until the signature has been reconstructed.
    /// Will timeout if the instance is cleaned up, see [`SIGNATURE_COLLECTOR_RETAIN_SLOTS`].
    /// Check the fields of the parameter structs for more info.
    /// The rough idea behind the separation is that `metadata` will be the same across all calls if
    /// we sign for all validators in a committee, while `validator_signing_data` varies for each.
    pub async fn sign_and_collect(
        self: &Arc<Self>,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        validator_signing_data: ValidatorSigningData,
    ) -> Result<Arc<Signature>, CollectionError> {
        let Some(signer) = self.operator_id.get() else {
            return Err(CollectionError::OwnOperatorIdUnknown);
        };

        let (result_tx, result_rx) = oneshot::channel();

        debug!(
            ?metadata,
            ?requester,
            root=?validator_signing_data.root,
            index=?validator_signing_data.index,
            "sign_and_collect called",
        );

        // first, register notifier with preexisting or newly spawned instance
        let cloned_metadata = metadata.clone();
        let manager = self.clone();
        self.processor.permitless.send_immediate(
            move |drop_on_finish| {
                let sender = manager.get_or_spawn(
                    validator_signing_data.root,
                    validator_signing_data.index,
                    cloned_metadata.slot,
                );
                let _ = sender.send(CollectorMessage {
                    kind: CollectorMessageKind::RegisterNotifier {
                        notify: result_tx,
                        threshold: cloned_metadata.threshold,
                    },
                    _drop_on_finish: drop_on_finish,
                });
            },
            COLLECTOR_MESSAGE_NAME,
        )?;

        // then, create the partial signature - and maybe send the message.
        let manager = self.clone();
        self.processor.urgent_consensus.send_blocking(
            move || {
                trace!(root = ?validator_signing_data.root, "Signing...");
                // If we have no share, we can not actually sign the message, because we are running
                // in impostor mode.
                let partial_signature = if let Some(share) = &validator_signing_data.share {
                    share.sign(validator_signing_data.root)
                } else {
                    Signature::empty()
                };
                trace!(root = ?validator_signing_data.root, "Signed");

                let message = PartialSignatureMessage {
                    partial_signature,
                    signing_root: validator_signing_data.root,
                    signer,
                    validator_index: validator_signing_data.index,
                };
                match requester {
                    SignatureRequester::SingleValidator { pubkey } => {
                        // we do not have to wait for other partial signatures - send the message
                        // immediately.
                        if let Err(err) = manager.message_sender.sign_and_send(
                            manager.create_message(
                                &metadata,
                                vec![message.clone()],
                                &DutyExecutor::Validator(pubkey),
                            ),
                            metadata.committee_id,
                            None,
                        ) {
                            error!(?err, "Error sending validator partial signature");
                        }
                    }
                    SignatureRequester::Committee {
                        num_signatures_to_collect,
                        base_hash,
                    } => {
                        // We have to collect all signatures from the given validators.
                        // To check this create or get an entry from the `committee_signatures` map.
                        let mut entry = match manager
                            .committee_signatures
                            .entry((base_hash, metadata.committee_id))
                        {
                            Entry::Occupied(occupied) => occupied,
                            Entry::Vacant(vacant) => vacant.insert_entry(CommitteeSignatures {
                                collected_signatures: Vec::with_capacity(num_signatures_to_collect),
                                for_slot: metadata.slot,
                            }),
                        };
                        let collected_signatures = &mut entry.get_mut().collected_signatures;

                        // Enter the signature we just signed for this validator.
                        collected_signatures.push(message.clone());

                        debug!(
                            have = collected_signatures.len(),
                            need = num_signatures_to_collect,
                            "Checking if we have all signatures to send"
                        );

                        // If we collected the correct number of signatures, create and sign the
                        // final message.
                        if collected_signatures.len() == num_signatures_to_collect {
                            let signatures = entry.remove().collected_signatures;

                            if let Err(err) = manager.message_sender.sign_and_send(
                                manager.create_message(
                                    &metadata,
                                    signatures,
                                    &DutyExecutor::Committee(metadata.committee_id),
                                ),
                                metadata.committee_id,
                                None,
                            ) {
                                error!(?err, "Error sending committee partial signatures");
                            }
                        }
                    }
                }

                // Finally, make the local instance aware of the partial signature, if it is a real
                // signature.
                if validator_signing_data.share.is_some() {
                    let _ = manager.receive_partial_signature(message, metadata.slot);
                }
            },
            SIGNER_NAME,
        )?;

        // We resolve the collector future - if we are lucky, the signature is even already done
        // because we received enough shares before this fn was even called.
        Ok(result_rx.await?)
    }

    fn create_message(
        &self,
        metadata: &SignatureMetadata,
        signatures: Vec<PartialSignatureMessage>,
        duty_executor: &DutyExecutor,
    ) -> UnsignedSSVMessage {
        let partial_sig_messages = PartialSignatureMessages {
            kind: metadata.kind,
            slot: metadata.slot,
            messages: signatures,
        };

        UnsignedSSVMessage {
            ssv_message: SSVMessage::new(
                MsgType::SSVPartialSignatureMsgType,
                MessageId::new(&self.domain, metadata.role, duty_executor),
                partial_sig_messages.as_ssz_bytes(),
            )
            .expect("Creating a SSVMessage must succeed"),
            full_data: vec![],
        }
    }

    pub fn receive_partial_signatures(
        self: &Arc<Self>,
        messages: PartialSignatureMessages,
    ) -> Result<(), CollectionError> {
        for message in messages.messages {
            self.receive_partial_signature(message, messages.slot)?;
        }
        Ok(())
    }

    fn receive_partial_signature(
        self: &Arc<Self>,
        message: PartialSignatureMessage,
        slot: Slot,
    ) -> Result<(), CollectionError> {
        debug!(
            ?slot,
            signing_root=?message.signing_root,
            signer=?message.signer,
            validator=?message.validator_index,
            "Received partial signature message",
        );
        let manager = self.clone();
        self.processor.permitless.send_immediate(
            move |drop_on_finish| {
                let sender =
                    manager.get_or_spawn(message.signing_root, message.validator_index, slot);
                if let Err(err) = sender.send(CollectorMessage {
                    kind: CollectorMessageKind::PartialSignature {
                        operator_id: message.signer,
                        signature: Box::new(message.partial_signature),
                    },
                    _drop_on_finish: drop_on_finish,
                }) {
                    error!(
                        ?err,
                        "failed to send partial signature to collector instance"
                    );
                }
            },
            COLLECTOR_MESSAGE_NAME,
        )?;
        Ok(())
    }

    fn get_or_spawn(
        &self,
        signing_root: Hash256,
        validator_index: ValidatorIndex,
        slot: Slot,
    ) -> UnboundedSender<CollectorMessage> {
        match self
            .signature_collectors
            .entry((signing_root, validator_index))
        {
            Entry::Occupied(entry) => entry.get().sender.clone(),
            Entry::Vacant(entry) => {
                // this channel is effectively limited by the processor permit amount
                let (tx, rx) = mpsc::unbounded_channel();
                let span = debug_span!(
                    "signature_collector",
                    ?slot,
                    ?validator_index,
                    ?signing_root
                );
                entry.insert(SignatureCollector {
                    sender: tx.clone(),
                    for_slot: slot,
                });
                let _ = self.processor.permitless.send_async(
                    Box::pin(signature_collector(rx).instrument(span)),
                    COLLECTOR_NAME,
                );
                debug!(
                    ?signing_root,
                    ?validator_index,
                    "Spawned signature collector"
                );
                tx
            }
        }
    }

    async fn cleaner(self: Arc<Self>, slot_clock: impl SlotClock) {
        while !self.processor.permitless.is_closed() {
            sleep(
                slot_clock
                    .duration_to_next_slot()
                    .unwrap_or(slot_clock.slot_duration()),
            )
            .await;
            let Some(slot) = slot_clock.now() else {
                continue;
            };
            let cutoff = slot.saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS);
            self.signature_collectors
                .retain(|_, collector| collector.for_slot >= cutoff);
            self.committee_signatures
                .retain(|_, signatures| signatures.for_slot >= cutoff);
        }
    }
}

/// Metadata around the signature(s) to create.
#[derive(Debug, Clone)]
pub struct SignatureMetadata {
    /// The signature kind to transmit. Only needed for the network message we send.
    pub kind: PartialSignatureKind,
    /// The role to transmit. Only needed for the network message we send.
    pub role: Role,
    /// The signature threshold amount after which we can reconstruct the full signature.
    pub threshold: u64,
    /// The slot relevant for this signature. The collector instance is cleaned up one slot after
    /// this. Also used in the network message.
    pub slot: Slot,
    /// The committee of the signer(s). Used in the created network message.
    pub committee_id: CommitteeId,
}

/// Who is signing this - only a single validator, or potentially multiple validators in a
/// committee? This is relevant because we send only one message in the latter case.
#[derive(Debug, Clone)]
pub enum SignatureRequester {
    /// The only validator signing this is the one passed when `sign_and_collect` is called.
    SingleValidator {
        /// The public key of the validator. Used in the created network message.
        pubkey: PublicKeyBytes,
    },
    /// We need to wait for all these validators to submit their signature until we can send.
    Committee {
        /// The number of signatures we have to wait for.
        num_signatures_to_collect: usize,
        /// A hash that identifies what we are signing. We wait with sending the message until we
        /// have created enough signatures with this `base_hash`. We need this to differentiate
        /// "groups" of signatures. We cannot use the signing root, as we need to group signatures
        /// with differing signing roots.
        base_hash: Hash256,
    },
}

#[derive(Clone)]
pub struct ValidatorSigningData {
    pub root: Hash256,
    pub index: ValidatorIndex,
    pub share: Option<SecretKey>,
}

struct CollectorMessage {
    kind: CollectorMessageKind,
    _drop_on_finish: DropOnFinish,
}

#[derive(Debug)]
enum CollectorMessageKind {
    /// A new task is waiting for the result of this collector instance.
    RegisterNotifier {
        notify: oneshot::Sender<Arc<Signature>>,
        threshold: u64,
    },
    /// A new partial signature is available - either because it arrived from the network, or
    /// because we created it
    PartialSignature {
        /// The signer.
        operator_id: OperatorId,
        /// The signature, boxed because else Clippy complains.
        signature: Box<Signature>,
    },
}

#[derive(Debug, Clone)]
pub enum CollectionError {
    QueueClosedError,
    QueueFullError,
    CollectionTimeout,
    EmptySignature,
    OwnOperatorIdUnknown,
    RecoverError(bls_lagrange::Error),
}

impl From<Error> for CollectionError {
    fn from(value: Error) -> Self {
        match value {
            Queue(TrySendError::Full(_)) => CollectionError::QueueFullError,
            Queue(TrySendError::Closed(_)) => CollectionError::QueueClosedError,
        }
    }
}

impl From<RecvError> for CollectionError {
    fn from(_: RecvError) -> Self {
        CollectionError::QueueClosedError
    }
}

impl From<bls_lagrange::Error> for CollectionError {
    fn from(err: bls_lagrange::Error) -> Self {
        CollectionError::RecoverError(err)
    }
}

/// The actual signature collector task, waiting for messages
async fn signature_collector(mut rx: mpsc::UnboundedReceiver<CollectorMessage>) {
    let mut notifiers = vec![];
    let mut signature_share = HashMap::new();
    let mut full_signature: Option<Arc<Signature>> = None;
    let mut threshold = None;

    while let Some(message) = rx.recv().await {
        trace!(msg=?message.kind, "Signature collector received message");
        match message.kind {
            CollectorMessageKind::RegisterNotifier {
                notify,
                threshold: new_threshold,
            } => {
                if let Some(full_signature) = &full_signature {
                    // We already got a reconstructed signature, send it immediately.
                    if let Err(err) = notify.send(full_signature.clone()) {
                        warn!(?err, "Failed to send recovered signature");
                    }
                } else {
                    // Register the notifier and threshold.
                    notifiers.push(notify);
                    if let Some(old_threshold) = threshold
                        && new_threshold != old_threshold
                    {
                        // Different tasks expect different thresholds. We can not know which is
                        // correct, so we exit this instance.
                        error!(
                            new_threshold,
                            old_threshold, "Conflicting thresholds passed!"
                        );
                        return;
                    }
                    threshold = Some(new_threshold);
                }
            }
            CollectorMessageKind::PartialSignature {
                operator_id,
                signature,
            } => {
                if full_signature.is_some() {
                    // Already got the full signature.
                    continue;
                }

                // Insert the signature into our map.
                match signature_share.entry(operator_id) {
                    hash_map::Entry::Vacant(entry) => {
                        entry.insert(*signature);
                    }
                    hash_map::Entry::Occupied(entry) => {
                        if entry.get() != &*signature {
                            // We can not know which signature is correct. This is serious
                            // misbehaviour from the operator!
                            error!(
                                ?operator_id,
                                "Received conflicting signatures from operator"
                            );
                        }
                    }
                }
            }
        }

        if let Some(threshold) = threshold
            && signature_share.len() as u64 >= threshold
        {
            let signature = match combine_signatures(mem::take(&mut signature_share)) {
                Ok(signature) => Arc::new(signature),
                Err(err) => {
                    error!(?err, "Failed to recover signature");
                    return;
                }
            };

            debug!(?signature, "Successfully recovered signature");

            for notifier in mem::take(&mut notifiers) {
                if notifier.send(Arc::clone(&signature)).is_err() {
                    warn!("Callback dropped - signature is no longer relevant");
                }
            }
            full_signature = Some(signature);
        }
    }
}

fn combine_signatures(
    shares: HashMap<OperatorId, Signature>,
) -> Result<Signature, CollectionError> {
    let (ids, signatures): (Vec<_>, Vec<_>) = shares
        .into_iter()
        .map(|(k, s)| KeyId::try_from(*k).map(|k| (k, s)))
        .collect::<Result<_, _>>()?;

    Ok(bls_lagrange::combine_signatures(&signatures, &ids)?)
}
