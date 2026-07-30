mod metrics;

use std::{
    collections::{HashMap, hash_map},
    future::Future,
    mem,
    ops::ControlFlow,
    pin::Pin,
    sync::Arc,
};

use bls::{PublicKey, PublicKeyBytes, SecretKey, Signature};
use bls_lagrange::KeyId;
use dashmap::{DashMap, Entry};
use database::{NetworkDatabase, OwnOperatorId};
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
        Semaphore, mpsc,
        mpsc::{UnboundedSender, error::TrySendError},
        oneshot,
        oneshot::error::RecvError,
    },
    task,
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

/// Locally accumulated validator partial signatures for one outgoing committee message.
/// As soon as this operator has produced the full validator batch for the committee round, the
/// message is sent.
struct CommitteePartialSignatureBatch {
    batched_validator_partial_signatures: Vec<PartialSignatureMessage>,
    for_slot: Slot,
}

pub struct SignatureCollectorManager<S: SlotClock> {
    /// The handle to the processor, for queueing messages to the instances.
    processor: Senders,
    /// The local operator we act for.
    operator_id: OwnOperatorId,
    /// Loads share public keys after a reconstruction failure.
    share_pubkey_loader: SharePubkeyLoader,
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
    /// A map from the hash of a decided committee value and committee ID to the local batch of
    /// validator partial signatures for that committee round.
    /// Note that this hash may differ from the actual signing root.
    committee_partial_signature_batches:
        DashMap<(Hash256, CommitteeId), CommitteePartialSignatureBatch>,
}

impl<S: SlotClock + Clone + 'static> SignatureCollectorManager<S> {
    pub fn new(
        processor: Senders,
        operator_id: OwnOperatorId,
        database: Arc<NetworkDatabase>,
        fork_schedule: Arc<ForkSchedule>,
        slots_per_epoch: u64,
        message_sender: Arc<dyn MessageSender>,
        slot_clock: S,
    ) -> Result<Arc<Self>, CollectionError> {
        let manager = Arc::new(Self {
            processor,
            operator_id,
            share_pubkey_loader: SharePubkeyLoader::new(database),
            fork_schedule,
            slot_clock: slot_clock.clone(),
            slots_per_epoch,
            message_sender,
            signature_collectors: DashMap::new(),
            committee_partial_signature_batches: DashMap::new(),
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
        let validator_pubkey = validator_signing_data.validator_pubkey;
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
                        validator_pubkey,
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
                    SignatureRequester::Committee {
                        validator_partial_signature_batch_size,
                        base_hash,
                    } => {
                        // Batch one locally produced partial signature per validator before
                        // sending a single committee message for this round.
                        let mut entry = match manager
                            .committee_partial_signature_batches
                            .entry((base_hash, metadata.committee_id))
                        {
                            Entry::Occupied(occupied) => occupied,
                            Entry::Vacant(vacant) => vacant.insert_entry(CommitteePartialSignatureBatch {
                                batched_validator_partial_signatures: Vec::with_capacity(
                                    validator_partial_signature_batch_size,
                                ),
                                for_slot: metadata.slot,
                            }),
                        };
                        let validator_partial_signature_batch =
                            &mut entry.get_mut().batched_validator_partial_signatures;

                        // Add the partial signature we just produced for this validator to the
                        // local batch.
                        validator_partial_signature_batch.push(message.clone());

                        trace!(
                            have = validator_partial_signature_batch.len(),
                            need = validator_partial_signature_batch_size,
                            "Checking whether the batch of validator partial signatures is ready to send"
                        );

                        // Once the local batch of validator partial signatures is complete,
                        // create and send the committee message.
                        if validator_partial_signature_batch.len()
                            == validator_partial_signature_batch_size
                        {
                            let signatures =
                                entry.remove().batched_validator_partial_signatures;

                            let msg = match manager.create_message(
                                &metadata,
                                signatures,
                                &DutyExecutor::Committee(metadata.committee_id),
                            ) {
                                Ok(msg) => msg,
                                Err(err) => {
                                    error!(%err, "Failed to create committee partial signature message");
                                    return;
                                }
                            };

                            if let Err(err) =
                                manager
                                    .message_sender
                                    .sign_and_send(msg, metadata.committee_id, None)
                            {
                                error!(?err, "Failed to send committee partial signatures");
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
                if sender
                    .send(CollectorMessage {
                        kind: CollectorMessageKind::PartialSignature {
                            operator_id: message.signer,
                            signature: Box::new(message.partial_signature),
                        },
                        _drop_on_finish: drop_on_finish,
                    })
                    .is_err()
                {
                    error!("Failed to send partial signature to collector instance");
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
                    Box::pin(
                        signature_collector(rx, signing_root, self.share_pubkey_loader.clone())
                            .instrument(span),
                    ),
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
            self.committee_partial_signature_batches
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
    pub validator_pubkey: PublicKeyBytes,
    pub share: Option<SecretKey>,
}

struct CollectorMessage<G = DropOnFinish> {
    kind: CollectorMessageKind,
    _drop_on_finish: G,
}

#[derive(Debug)]
enum CollectorMessageKind {
    /// A new task is waiting for the result of this collector instance.
    RegisterNotifier {
        notify: oneshot::Sender<Arc<Signature>>,
        threshold: u64,
        validator_pubkey: PublicKeyBytes,
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
async fn signature_collector<G: Send + 'static>(
    mut rx: mpsc::UnboundedReceiver<CollectorMessage<G>>,
    signing_root: Hash256,
    share_pubkey_loader: SharePubkeyLoader,
) {
    let mut state = SignatureCollectorState::new(signing_root);

    while let Some(message) = rx.recv().await {
        match message.kind {
            CollectorMessageKind::RegisterNotifier {
                notify,
                threshold,
                validator_pubkey,
            } => {
                if state
                    .register_request(notify, threshold, validator_pubkey)
                    .is_break()
                {
                    return;
                }
            }
            CollectorMessageKind::PartialSignature {
                operator_id,
                signature,
            } => {
                state.add_partial_signature(operator_id, *signature);
            }
        }

        match state.try_reconstruct() {
            Ok(None) => {}
            Ok(Some(signature)) => state.complete_reconstruction(signature),
            Err(failure) => {
                if handle_reconstruction_fallback(&mut state, failure, &share_pubkey_loader)
                    .await
                    .is_break()
                {
                    return;
                }
            }
        }
    }
}

async fn handle_reconstruction_fallback(
    state: &mut SignatureCollectorState,
    failure: ReconstructionFailure,
    share_pubkey_loader: &SharePubkeyLoader,
) -> ControlFlow<()> {
    metrics::inc_counter(&metrics::RECONSTRUCTION_FALLBACKS_TOTAL);

    let Some(registration) = state.registration.as_ref() else {
        error!("Reconstruction fallback requested before registration");
        return ControlFlow::Break(());
    };
    let validator_pubkey = registration.validator_pubkey;
    let threshold = registration.threshold;
    let (failure_kind, combination_error) = match &failure {
        ReconstructionFailure::Combination(err) => ("combination", Some(err)),
        ReconstructionFailure::MasterVerification => ("master_verification", None),
    };

    warn!(
        failure_kind,
        ?combination_error,
        share_count = state.signature_share.len(),
        threshold,
        "Reconstructed signature failed, verifying buffered shares"
    );

    let Some(share_pubkeys) = share_pubkey_loader.fetch(validator_pubkey).await else {
        return ControlFlow::Break(());
    };

    let invalid_operator_ids = state.remove_invalid_shares(&share_pubkeys);
    let remaining_count = state.signature_share.len();
    warn!(
        ?invalid_operator_ids,
        invalid_count = invalid_operator_ids.len(),
        remaining_count,
        threshold,
        "Verified buffered shares after reconstruction failure"
    );

    if invalid_operator_ids.is_empty() {
        error!(
            remaining_count,
            threshold, "Reconstruction failed although every buffered share verified"
        );
        return ControlFlow::Break(());
    }

    if (remaining_count as u64) < threshold {
        return ControlFlow::Continue(());
    }

    match state.try_reconstruct() {
        Ok(Some(signature)) => {
            state.complete_reconstruction(signature);
            ControlFlow::Continue(())
        }
        Ok(None) => {
            error!(
                remaining_count,
                threshold, "Immediate reconstruction retry did not complete"
            );
            ControlFlow::Break(())
        }
        Err(failure) => {
            error!(
                ?failure,
                remaining_count, threshold, "Immediate reconstruction retry failed"
            );
            ControlFlow::Break(())
        }
    }
}

/// Loads all share public keys of a validator from the database, serializing
/// lookups before they enter Tokio's blocking pool.
#[derive(Clone)]
struct SharePubkeyLoader {
    database: Arc<NetworkDatabase>,
    semaphore: Arc<Semaphore>,
}

impl SharePubkeyLoader {
    fn new(database: Arc<NetworkDatabase>) -> Self {
        Self {
            database,
            semaphore: Arc::new(Semaphore::new(1)),
        }
    }

    async fn fetch(
        &self,
        validator_pubkey: PublicKeyBytes,
    ) -> Option<HashMap<OperatorId, PublicKeyBytes>> {
        let Ok(permit) = Arc::clone(&self.semaphore).acquire_owned().await else {
            error!("Signature reconstruction fallback semaphore closed");
            return None;
        };

        let database = Arc::clone(&self.database);
        match task::spawn_blocking(move || {
            let _permit = permit;
            database.get_share_pubkeys_for_validator(&validator_pubkey)
        })
        .await
        {
            Ok(Ok(share_pubkeys)) if share_pubkeys.is_empty() => {
                error!("Validator share public-key lookup returned no keys");
                None
            }
            Ok(Ok(share_pubkeys)) => Some(share_pubkeys),
            Ok(Err(err)) => {
                error!(%err, "Failed to load validator share public keys");
                None
            }
            Err(err) => {
                error!(%err, "Validator share public-key lookup task failed");
                None
            }
        }
    }
}

#[derive(Debug)]
enum ReconstructionFailure {
    Combination(CollectionError),
    MasterVerification,
}

struct CollectorRegistration {
    threshold: u64,
    validator_pubkey: PublicKeyBytes,
    decompressed_validator_pubkey: PublicKey,
}

/// Invariant: once `full_signature` is `Some`, both `signature_share` and
/// `notifiers` are empty (drained by `complete_reconstruction`).
struct SignatureCollectorState {
    signing_root: Hash256,
    notifiers: Vec<oneshot::Sender<Arc<Signature>>>,
    signature_share: HashMap<OperatorId, Signature>,
    full_signature: Option<Arc<Signature>>,
    registration: Option<CollectorRegistration>,
}

impl SignatureCollectorState {
    fn new(signing_root: Hash256) -> Self {
        Self {
            signing_root,
            notifiers: vec![],
            signature_share: HashMap::new(),
            full_signature: None,
            registration: None,
        }
    }

    /// Register a task waiting for the reconstructed signature.
    ///
    /// The first registration fixes the threshold and validator master public
    /// key. Later registrations must match both. A matching caller receives a
    /// cached verified signature immediately, or is queued until reconstruction
    /// completes.
    ///
    /// Returns `Break` if the master public key is malformed or a later
    /// registration conflicts. The recv loop then drops the state, closing all
    /// queued notifiers.
    fn register_request(
        &mut self,
        notify: oneshot::Sender<Arc<Signature>>,
        new_threshold: u64,
        validator_pubkey: PublicKeyBytes,
    ) -> ControlFlow<()> {
        if let Some(registration) = &self.registration {
            if new_threshold != registration.threshold
                || validator_pubkey != registration.validator_pubkey
            {
                error!(
                    new_threshold,
                    old_threshold = registration.threshold,
                    validator_pubkey_matches = validator_pubkey == registration.validator_pubkey,
                    "Conflicting registration passed to signature collector"
                );
                return ControlFlow::Break(());
            }
        } else {
            let decompressed_validator_pubkey = match validator_pubkey.decompress() {
                Ok(public_key) => public_key,
                Err(err) => {
                    error!(?err, "Failed to decompress validator public key");
                    return ControlFlow::Break(());
                }
            };
            self.registration = Some(CollectorRegistration {
                threshold: new_threshold,
                validator_pubkey,
                decompressed_validator_pubkey,
            });
        }

        if let Some(full_signature) = &self.full_signature {
            if notify.send(Arc::clone(full_signature)).is_err() {
                warn!("Failed to send recovered signature");
            }
            return ControlFlow::Continue(());
        }

        self.notifiers.push(notify);
        ControlFlow::Continue(())
    }

    /// Ingest a partial signature from one operator.
    ///
    /// Late shares arriving after reconstruction are silently dropped.
    /// Conflicting shares from the same operator are logged but not fatal,
    /// since the source of the discrepancy is not knowable here.
    fn add_partial_signature(&mut self, operator_id: OperatorId, signature: Signature) {
        if self.full_signature.is_some() {
            return;
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
    }

    fn try_reconstruct(&self) -> Result<Option<Signature>, ReconstructionFailure> {
        if self.full_signature.is_some() {
            return Ok(None);
        }
        let Some(registration) = &self.registration else {
            return Ok(None);
        };
        if (self.signature_share.len() as u64) < registration.threshold {
            return Ok(None);
        }

        let signature = combine_signatures(&self.signature_share)
            .map_err(ReconstructionFailure::Combination)?;

        if !signature.verify(
            &registration.decompressed_validator_pubkey,
            self.signing_root,
        ) {
            return Err(ReconstructionFailure::MasterVerification);
        }

        Ok(Some(signature))
    }

    fn complete_reconstruction(&mut self, signature: Signature) {
        trace!("Successfully recovered and verified signature");
        let signature = Arc::new(signature);
        self.signature_share.clear();

        for notifier in mem::take(&mut self.notifiers) {
            if notifier.send(Arc::clone(&signature)).is_err() {
                warn!("Callback dropped, signature is no longer relevant");
            }
        }
        self.full_signature = Some(signature);
    }

    fn remove_invalid_shares(
        &mut self,
        share_pubkeys: &HashMap<OperatorId, PublicKeyBytes>,
    ) -> Vec<OperatorId> {
        let mut invalid_operator_ids = vec![];
        self.signature_share.retain(|operator_id, signature| {
            let is_valid = share_pubkeys
                .get(operator_id)
                .and_then(|public_key| public_key.decompress().ok())
                .is_some_and(|public_key| signature.verify(&public_key, self.signing_root));
            if !is_valid {
                invalid_operator_ids.push(*operator_id);
            }
            is_valid
        });
        invalid_operator_ids.sort_unstable();
        invalid_operator_ids
    }
}

fn combine_signatures(
    shares: &HashMap<OperatorId, Signature>,
) -> Result<Signature, CollectionError> {
    let (ids, signatures): (Vec<_>, Vec<_>) = shares
        .iter()
        .map(|(operator_id, signature)| {
            KeyId::try_from(**operator_id).map(|key_id| (key_id, signature.clone()))
        })
        .collect::<Result<_, _>>()?;

    Ok(bls_lagrange::combine_signatures(&signatures, &ids)?)
}

#[cfg(test)]
mod tests;
