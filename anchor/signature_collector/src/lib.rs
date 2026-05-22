use std::{
    collections::{HashMap, hash_map},
    future::Future,
    mem,
    ops::ControlFlow,
    pin::Pin,
    sync::Arc,
};

use bls::{PublicKeyBytes, SecretKey, Signature};
use bls_lagrange::KeyId;
use dashmap::{DashMap, Entry};
use database::OwnOperatorId;
use fork::ForkSchedule;
use message_sender::MessageSender;
use processor::{Error, Error::Queue, Senders, work::DropOnFinish};
use slot_clock::SlotClock;
use ssv_types::typenum::Unsigned;
pub use ssv_types::{
    CommitteeId, OperatorId, ValidatorIndex,
    consensus::UnsignedSSVMessage,
    domain_type::DomainType,
    message::{MsgType, SSVMessage, SSVMessageError},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::{
        PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages,
        PartialSignatureMessagesLen,
    },
};
use ssz::Encode;
use thiserror::Error;
use tokio::{
    sync::{
        mpsc,
        mpsc::{UnboundedSender, error::TrySendError},
        oneshot,
        oneshot::error::RecvError,
    },
    time::sleep,
};
use tracing::{Instrument, debug_span, error, trace, warn};
use types::{Hash256, Slot};

const COLLECTOR_NAME: &str = "signature_collector";
const COLLECTOR_MESSAGE_NAME: &str = "signature_collector_message";
const COLLECTOR_CLEANER_NAME: &str = "signature_collector_cleaner";
const SIGNER_NAME: &str = "partial_signer";

/// number of slots to keep before the current slot
const SIGNATURE_COLLECTOR_RETAIN_SLOTS: u64 = 1;

/// Error type for creating partial signature messages.
#[derive(Debug, Error)]
enum CreateMessageError {
    #[error("Too many partial signatures: {count} exceeds maximum {max}")]
    TooManySignatures { count: usize, max: usize },
    #[error("Failed to create SSV message: {0}")]
    SSVMessage(#[from] SSVMessageError),
}

/// A handle to message the instance collecting a single specific signature
struct SignatureCollector {
    sender: UnboundedSender<CollectorMessage>,
    for_slot: Slot,
}

/// Locally accumulated validator partial signatures for one outgoing message.
/// As soon as this operator has produced the full validator batch for the duty, the message is
/// sent.
struct PartialSignatureBatch {
    batched_validator_partial_signatures: Vec<PartialSignatureMessage>,
    for_slot: Slot,
}

pub struct SignatureCollectorManager<S: SlotClock> {
    /// The handle to the processor, for queueing messages to the instances.
    processor: Senders,
    /// The local operator we act for.
    operator_id: OwnOperatorId,
    /// The fork schedule for looking up the slot-based domain type.
    fork_schedule: Arc<ForkSchedule>,
    /// The slot clock for determining the current epoch.
    slot_clock: S,
    /// Number of slots per epoch (needed for epoch calculation).
    slots_per_epoch: u64,
    /// A message sender used for outgoing messages.
    message_sender: Arc<dyn MessageSender>,
    /// A map from the signing root and signing validator to the corresponding signature collector.
    signature_collectors: DashMap<(Hash256, ValidatorIndex), SignatureCollector>,
    /// A map from a caller-provided batch ID and duty executor to the local batch of validator
    /// partial signatures for one outgoing message. The batch ID may differ from the actual signing
    /// root when the outgoing message contains signatures over multiple roots.
    partial_signature_batches: DashMap<(Hash256, DutyExecutor), PartialSignatureBatch>,
}

impl<S: SlotClock + Clone + 'static> SignatureCollectorManager<S> {
    pub fn new(
        processor: Senders,
        operator_id: OwnOperatorId,
        fork_schedule: Arc<ForkSchedule>,
        slots_per_epoch: u64,
        message_sender: Arc<dyn MessageSender>,
        slot_clock: S,
    ) -> Result<Arc<Self>, CollectionError> {
        let manager = Arc::new(Self {
            processor,
            operator_id,
            fork_schedule,
            slot_clock: slot_clock.clone(),
            slots_per_epoch,
            message_sender,
            signature_collectors: DashMap::new(),
            partial_signature_batches: DashMap::new(),
        });

        manager
            .processor
            .permitless
            .send_async(Arc::clone(&manager).cleaner(), COLLECTOR_CLEANER_NAME)?;

        Ok(manager)
    }

    /// Get the domain type for a message slot.
    fn domain_type_for_slot(&self, slot: Slot) -> DomainType {
        let epoch = slot.epoch(self.slots_per_epoch);
        self.fork_schedule.active_fork_config(epoch).domain_type
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

        trace!(
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
                        let msg = match manager.create_message(
                            &metadata,
                            vec![message.clone()],
                            &DutyExecutor::Validator(pubkey),
                        ) {
                            Ok(msg) => msg,
                            Err(err) => {
                                error!(%err, "Failed to create validator partial signature message");
                                return;
                            }
                        };

                        if let Err(err) =
                            manager
                                .message_sender
                                .sign_and_send(msg, metadata.committee_id, None)
                        {
                            error!(?err, "Failed to send validator partial signature");
                        }
                    }
                    SignatureRequester::SingleValidatorBatch {
                        pubkey,
                        validator_partial_signature_batch_size,
                        base_hash,
                    } => manager.send_batched_partial_signature(
                        &metadata,
                        DutyExecutor::Validator(pubkey),
                        validator_partial_signature_batch_size,
                        base_hash,
                        message.clone(),
                    ),
                    SignatureRequester::Committee {
                        validator_partial_signature_batch_size,
                        base_hash,
                    } => manager.send_batched_partial_signature(
                        &metadata,
                        DutyExecutor::Committee(metadata.committee_id),
                        validator_partial_signature_batch_size,
                        base_hash,
                        message.clone(),
                    ),
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

    fn send_batched_partial_signature(
        &self,
        metadata: &SignatureMetadata,
        duty_executor: DutyExecutor,
        validator_partial_signature_batch_size: usize,
        base_hash: Hash256,
        message: PartialSignatureMessage,
    ) {
        if validator_partial_signature_batch_size == 0 {
            error!("Cannot create a partial signature batch with zero expected signatures");
            return;
        }

        let mut entry = match self
            .partial_signature_batches
            .entry((base_hash, duty_executor.clone()))
        {
            Entry::Occupied(occupied) => occupied,
            Entry::Vacant(vacant) => vacant.insert_entry(PartialSignatureBatch {
                batched_validator_partial_signatures: Vec::with_capacity(
                    validator_partial_signature_batch_size,
                ),
                for_slot: metadata.slot,
            }),
        };

        let batch_is_ready = {
            let validator_partial_signature_batch =
                &mut entry.get_mut().batched_validator_partial_signatures;
            validator_partial_signature_batch.push(message);

            trace!(
                have = validator_partial_signature_batch.len(),
                need = validator_partial_signature_batch_size,
                "Checking whether the batch of validator partial signatures is ready to send"
            );

            validator_partial_signature_batch.len() == validator_partial_signature_batch_size
        };

        if !batch_is_ready {
            return;
        }

        let signatures = entry.remove().batched_validator_partial_signatures;

        let msg = match self.create_message(metadata, signatures, &duty_executor) {
            Ok(msg) => msg,
            Err(err) => {
                error!(%err, "Failed to create batched partial signature message");
                return;
            }
        };

        if let Err(err) = self
            .message_sender
            .sign_and_send(msg, metadata.committee_id, None)
        {
            error!(?err, "Failed to send batched partial signatures");
        }
    }

    fn create_message(
        &self,
        metadata: &SignatureMetadata,
        signatures: Vec<PartialSignatureMessage>,
        duty_executor: &DutyExecutor,
    ) -> Result<UnsignedSSVMessage, CreateMessageError> {
        let domain = self.domain_type_for_slot(metadata.slot);
        let count = signatures.len();
        let messages = ssv_types::VariableList::new(signatures).map_err(|_| {
            CreateMessageError::TooManySignatures {
                count,
                max: PartialSignatureMessagesLen::USIZE,
            }
        })?;

        let partial_sig_messages = PartialSignatureMessages {
            kind: metadata.kind,
            slot: metadata.slot,
            messages,
        };

        Ok(UnsignedSSVMessage {
            ssv_message: SSVMessage::new(
                MsgType::SSVPartialSignatureMsgType,
                MessageId::new(&domain, metadata.role, duty_executor),
                partial_sig_messages.as_ssz_bytes(),
            )?,
            full_data: vec![],
        })
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
        trace!(
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
                trace!(
                    ?signing_root,
                    ?validator_index,
                    "Spawned signature collector"
                );
                tx
            }
        }
    }

    async fn cleaner(self: Arc<Self>) {
        let slot_clock = &self.slot_clock;
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
            self.partial_signature_batches
                .retain(|_, batch| batch.for_slot >= cutoff);
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
    /// The threshold of operator shares used by the per-validator reconstruction collector.
    /// Once partial signatures from this many operators have arrived over the network, the full
    /// validator signature can be reconstructed.
    ///
    /// This is distinct from
    /// `SignatureRequester::Committee::validator_partial_signature_batch_size`, which only
    /// controls how many validator partial signatures this operator batches locally before sending
    /// its own committee message.
    pub threshold: u64,
    /// The slot relevant for this signature. The collector instance is cleaned up one slot after
    /// this. Also used in the network message.
    pub slot: Slot,
    /// The committee of the signer(s). Used in the created network message.
    pub committee_id: CommitteeId,
}

/// Describes whether this request signs for a single validator or for multiple validators in a
/// committee. This matters because the committee case sends one message for the whole batch.
#[derive(Debug, Clone)]
pub enum SignatureRequester {
    /// The only validator signing this is the one passed when `sign_and_collect` is called.
    SingleValidator {
        /// The public key of the validator. Used in the created network message.
        pubkey: PublicKeyBytes,
    },
    /// The local operator is signing multiple roots for a single validator in one duty.
    /// We batch those validator partial signatures into a single outgoing validator message.
    SingleValidatorBatch {
        /// The public key of the validator. Used in the created network message.
        pubkey: PublicKeyBytes,
        /// How many validator partial signatures this operator must produce locally before sending
        /// the batched validator message.
        validator_partial_signature_batch_size: usize,
        /// Identifies which partial signatures belong in the same outgoing validator message.
        /// We cannot use the signing root because the batched signatures have different roots.
        base_hash: Hash256,
    },
    /// The local operator is signing for multiple validators in one committee round.
    /// We batch those validator partial signatures into a single outgoing committee message instead
    /// of sending one message per validator.
    Committee {
        /// How many validator partial signatures this operator must produce locally before sending
        /// the batched committee message.
        ///
        /// This is a local batching count, not the network reconstruction threshold from
        /// `SignatureMetadata::threshold`.
        validator_partial_signature_batch_size: usize,
        /// Identifies which partial signatures belong in the same outgoing committee message.
        /// We cannot use the signing root because the batched signatures may have different
        /// signing roots.
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

/// Trait abstracting signature collection for testability.
///
/// Production code uses `Arc<SignatureCollectorManager<S>>` which implements this trait.
/// Tests can provide a mock that returns canned signatures or errors.
pub trait SignatureCollecting: Send + Sync {
    fn sign_and_collect(
        &self,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        signing_data: ValidatorSigningData,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<Signature>, CollectionError>> + Send + '_>>;
}

impl<S: SlotClock + Clone + 'static> SignatureCollecting for Arc<SignatureCollectorManager<S>> {
    fn sign_and_collect(
        &self,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        signing_data: ValidatorSigningData,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<Signature>, CollectionError>> + Send + '_>> {
        Box::pin(SignatureCollectorManager::sign_and_collect(
            self,
            metadata,
            requester,
            signing_data,
        ))
    }
}

/// The actual signature collector task, waiting for messages.
///
/// The recv loop is the only place that matches on [`CollectorMessageKind`];
/// it dispatches each transport message to a typed method on
/// [`SignatureCollectorState`] so the state machine never sees the channel
/// shape (or the size-balancing `Box<Signature>` it carries).
async fn signature_collector(mut rx: mpsc::UnboundedReceiver<CollectorMessage>) {
    let mut state = SignatureCollectorState::default();
    while let Some(message) = rx.recv().await {
        trace!(msg=?message.kind, "Signature collector received message");
        let outcome = match message.kind {
            CollectorMessageKind::RegisterNotifier { notify, threshold } => {
                state.register_request(notify, threshold)
            }
            CollectorMessageKind::PartialSignature {
                operator_id,
                signature,
            } => state.add_partial_signature(operator_id, *signature),
        };
        if outcome.is_break() {
            return;
        }
    }
}

/// Invariant: once `full_signature` is `Some`, both `signature_share` and
/// `notifiers` are empty (drained by `try_reconstruct`).
#[derive(Default)]
struct SignatureCollectorState {
    notifiers: Vec<oneshot::Sender<Arc<Signature>>>,
    signature_share: HashMap<OperatorId, Signature>,
    full_signature: Option<Arc<Signature>>,
    threshold: Option<u64>,
}

impl SignatureCollectorState {
    /// Register a task waiting for the reconstructed signature.
    ///
    /// If reconstruction has already completed, the cached signature is
    /// delivered immediately. Otherwise the notifier is queued and the
    /// threshold is recorded; conflicting thresholds from concurrent
    /// requests cause the collector to `Break` (the recv loop drops the
    /// state, surfacing `RecvError` to every waiter).
    ///
    /// Returns `Break` when the collector should exit (conflicting
    /// thresholds, unrecoverable reconstruction failure).
    fn register_request(
        &mut self,
        notify: oneshot::Sender<Arc<Signature>>,
        new_threshold: u64,
    ) -> ControlFlow<()> {
        if let Some(full_signature) = &self.full_signature {
            if let Err(err) = notify.send(Arc::clone(full_signature)) {
                warn!(?err, "Failed to send recovered signature");
            }
            return ControlFlow::Continue(());
        }
        self.notifiers.push(notify);
        if let Some(old_threshold) = self.threshold
            && new_threshold != old_threshold
        {
            // Different tasks expect different thresholds. We can not know which is
            // correct, so we exit this instance.
            error!(
                new_threshold,
                old_threshold, "Conflicting thresholds passed!"
            );
            return ControlFlow::Break(());
        }
        self.threshold = Some(new_threshold);
        self.try_reconstruct()
    }

    /// Ingest a partial signature from one operator.
    ///
    /// Late shares arriving after reconstruction are silently dropped.
    /// Conflicting shares from the same operator are logged but not fatal,
    /// since the source of the discrepancy is not knowable here.
    ///
    /// Returns `Break` when the collector should exit (unrecoverable
    /// reconstruction failure).
    fn add_partial_signature(
        &mut self,
        operator_id: OperatorId,
        signature: Signature,
    ) -> ControlFlow<()> {
        if self.full_signature.is_some() {
            return ControlFlow::Continue(());
        }

        match self.signature_share.entry(operator_id) {
            hash_map::Entry::Vacant(entry) => {
                entry.insert(signature);
            }
            hash_map::Entry::Occupied(entry) => {
                if entry.get() != &signature {
                    // We can not know which signature is correct. This is serious
                    // misbehaviour from the operator!
                    error!(
                        ?operator_id,
                        "Received conflicting signatures from operator"
                    );
                }
            }
        }

        self.try_reconstruct()
    }

    fn try_reconstruct(&mut self) -> ControlFlow<()> {
        let Some(threshold) = self.threshold else {
            return ControlFlow::Continue(());
        };
        if (self.signature_share.len() as u64) < threshold {
            return ControlFlow::Continue(());
        }

        let signature = match combine_signatures(mem::take(&mut self.signature_share)) {
            Ok(signature) => Arc::new(signature),
            Err(err) => {
                error!(?err, "Failed to recover signature");
                return ControlFlow::Break(());
            }
        };

        trace!(?signature, "Successfully recovered signature");

        for notifier in mem::take(&mut self.notifiers) {
            if notifier.send(Arc::clone(&signature)).is_err() {
                warn!("Callback dropped - signature is no longer relevant");
            }
        }
        self.full_signature = Some(signature);
        ControlFlow::Continue(())
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

#[cfg(test)]
mod tests;
