//! Shared test infrastructure for `AnchorValidatorStore` integration tests.
//!
//! Provides a `ValidatorStoreTestHarness` that wires up a real `AnchorValidatorStore` with
//! in-memory database, mock consensus, and a mock signature collector.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use bls::{AggregateSignature, FixedBytesExtended, PublicKeyBytes, Signature};
use database::{NetworkDatabase, PendingStateUpdates};
use fork::{Fork, ForkSchedule};
use futures::StreamExt;
use message_sender::Error as SendError;
use parking_lot::Mutex;
use qbft::Completed;
use qbft_manager::{ConsensusDecider, QbftDecidable, QbftError, TimeoutMode};
use signature_collector::{
    CollectionError, CommitteeSelectionBatch, SignatureCollecting, SignatureMetadata,
    SignatureRequester, ValidatorSigningData,
};
use slashing_protection::SlashingDatabase;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    Cluster, ClusterId, CommitteeId, ENCRYPTED_KEY_LENGTH, IndexSet, OperatorId, Share,
    ValidatorIndex, ValidatorMetadata, VariableList,
    consensus::{AggregatorCommitteeConsensusData, QbftDataValidator, UnsignedSSVMessage},
    domain_type::DomainType,
    message::{MsgType, SSVMessage},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
};
use ssz::Encode;
use task_executor::TaskExecutor;
use tempfile::TempDir;
use tokio::{
    sync::{Barrier, watch},
    time::Instant,
};
use types::{
    Attestation, AttestationBase, AttestationData, ChainSpec, Checkpoint, Epoch, EthSpec, Graffiti,
    Hash256, MainnetEthSpec, SelectionProof, SignedAggregateAndProof, SignedContributionAndProof,
    Slot, SyncCommitteeContribution, SyncSelectionProof, SyncSubnetId,
};
use validator_store::{AggregateToSign, AttestationToSign, ContributionToSign, ValidatorStore};

use crate::{
    AggregationAssignments, AnchorValidatorStore, Error, VotingAssignments, VotingContext,
    aggregator_post_consensus::AggregatorPostConsensusShared,
};

pub(super) const TEST_SLOT: u64 = 1;
pub(super) const SLOT_DURATION_SECS: u64 = 12;
/// How far into `TEST_SLOT` the harness slot clock sits (just past the 1/3 mark).
pub(super) const CLOCK_OFFSET_INTO_TEST_SLOT_SECS: u64 = SLOT_DURATION_SECS / 3 + 1;
/// Bound on any Lighthouse callback stream in these tests; a callback that blocks past it is a
/// failure, not a slow machine.
pub(super) const STREAM_TIMEOUT: Duration = Duration::from_secs(5);
/// How long after a publication at the harness clock its Boole selection deadline (two thirds
/// into the slot) falls. The committee selection executions it starts send nothing after that.
pub(super) const SELECTION_DEADLINE_AFTER_PUBLICATION: Duration =
    Duration::from_secs(SLOT_DURATION_SECS * 2 / 3 - CLOCK_OFFSET_INTO_TEST_SLOT_SECS);
/// How long [`run_past_selection_deadline`] keeps watching after the deadline, for sends or
/// signings that should not happen.
const WATCH_PAST_SELECTION_DEADLINE: Duration = Duration::from_secs(2);

/// Advances past the selection deadline of a publication made at the harness clock, and on for a
/// watch period, so everything that publication's executions send has been sent. Only meant for
/// paused time, where it is instant.
pub(super) async fn run_past_selection_deadline() {
    tokio::time::sleep(SELECTION_DEADLINE_AFTER_PUBLICATION + WATCH_PAST_SELECTION_DEADLINE).await;
}

/// What the Lighthouse aggregate callback yields: one result per stream item.
pub(super) type SignAggregatesResult =
    Vec<Result<Vec<SignedAggregateAndProof<MainnetEthSpec>>, Error>>;

/// What the Lighthouse contribution callback yields: one result per stream item.
pub(super) type SignContributionsResult =
    Vec<Result<Vec<SignedContributionAndProof<MainnetEthSpec>>, Error>>;

// ==================== Mock consensus decider ====================

/// Mock that either echoes the proposed data or returns one fixed SSZ-decoded value.
/// Removes the need for `QbftManager` infrastructure and lets the signing pipeline run fully.
#[derive(Default)]
pub(super) struct MockConsensusDecider {
    fixed_decision: Option<(Vec<u8>, Arc<Barrier>)>,
}

impl MockConsensusDecider {
    pub(super) fn fixed_after_barrier<D: Encode>(value: &D, parties: usize) -> Self {
        Self {
            fixed_decision: Some((value.as_ssz_bytes(), Arc::new(Barrier::new(parties)))),
        }
    }
}

impl<E: EthSpec> ConsensusDecider<E> for MockConsensusDecider {
    async fn decide_instance<D: QbftDecidable<E>>(
        &self,
        _id: D::Id,
        initial: D,
        _validator: Box<dyn QbftDataValidator<D>>,
        _timeout_mode: TimeoutMode,
        _committee_members: &IndexSet<OperatorId>,
    ) -> Result<Completed<D>, QbftError> {
        let decided = match &self.fixed_decision {
            Some((bytes, barrier)) => {
                barrier.wait().await;
                D::from_ssz_bytes(bytes).expect("fixed test consensus value should decode")
            }
            None => initial,
        };
        Ok(Completed::Success(decided))
    }
}

// ==================== Mock signature collector ====================

/// Shared storage for captured `sign_and_collect` calls.
pub(super) type CapturedCalls = Arc<Mutex<Vec<CapturedSignatureCall>>>;

pub(super) struct CapturedSignatureCall {
    pub(super) metadata: SignatureMetadata,
    pub(super) requester: SignatureRequester,
    pub(super) validator_pubkey: PublicKeyBytes,
    pub(super) signing_root: Hash256,
    /// When the call was made.
    pub(super) captured_at: Instant,
}

/// Validator pubkeys whose signature collection fails; shared with the harness so tests can fail
/// a single validator's roots.
type FailingPubkeys = Arc<Mutex<HashSet<PublicKeyBytes>>>;

/// Mock that captures calls and returns a canned infinity signature, or a collection timeout for
/// every call once [`ValidatorStoreTestHarness::fail_signature_collection`] is called, or for one
/// validator's calls once [`ValidatorStoreTestHarness::fail_signature_collection_for`] is, or
/// never resolves at all once [`ValidatorStoreTestHarness::hang_signature_collection`] is.
///
/// Failing and hanging are different: a failure resolves to an error, which readers count as
/// `other_error`, while a hang leaves the root pending so the reader's own deadline decides its
/// fate. Only the hang mode reaches a per-root deadline-expiry path.
///
/// Boole committee selection signing and sending go to [`CommitteeSelectionMock`] instead.
///
/// Every field is shared, so the harness keeps a clone as its test-side handles.
#[derive(Clone)]
struct MockSignatureCollector {
    captured: CapturedCalls,
    fails: Arc<AtomicBool>,
    hangs: Arc<AtomicBool>,
    failing_pubkeys: FailingPubkeys,
    committee_selection: Arc<CommitteeSelectionMock>,
}

impl MockSignatureCollector {
    fn new(our_operator_id: OperatorId) -> Self {
        Self {
            captured: Arc::new(Mutex::new(Vec::new())),
            fails: Arc::new(AtomicBool::new(false)),
            hangs: Arc::new(AtomicBool::new(false)),
            failing_pubkeys: Arc::new(Mutex::new(HashSet::new())),
            committee_selection: Arc::new(CommitteeSelectionMock::new(our_operator_id)),
        }
    }
}

impl SignatureCollecting for MockSignatureCollector {
    fn sign_and_collect(
        &self,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        signing_data: ValidatorSigningData,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<Signature>, CollectionError>> + Send + '_>> {
        self.captured.lock().push(CapturedSignatureCall {
            metadata,
            requester,
            validator_pubkey: signing_data.validator_pubkey,
            signing_root: signing_data.root,
            captured_at: Instant::now(),
        });
        // Stands in for any root that never reaches quorum; the caller only distinguishes
        // success from failure.
        if self.fails.load(Ordering::Relaxed)
            || self
                .failing_pubkeys
                .lock()
                .contains(&signing_data.validator_pubkey)
        {
            return Box::pin(async { Err(CollectionError::CollectionTimeout) });
        }
        // Stands in for a root whose quorum never arrives: the future stays pending, so the
        // caller's deadline is what ends the wait.
        if self.hangs.load(Ordering::Relaxed) {
            return Box::pin(std::future::pending());
        }
        let sig = Signature::infinity().expect("infinity signature");
        Box::pin(async move { Ok(Arc::new(sig)) })
    }

    fn sign_committee_selection(
        &self,
        batch: Arc<CommitteeSelectionBatch>,
    ) -> Pin<Box<dyn Future<Output = Result<UnsignedSSVMessage, CollectionError>> + Send + '_>>
    {
        Box::pin(self.committee_selection.sign(batch))
    }

    fn send_committee_message(
        &self,
        message: UnsignedSSVMessage,
        committee_id: CommitteeId,
    ) -> Result<(), SendError> {
        self.committee_selection.send(message, committee_id)
    }
}

// ==================== Mock committee selection ====================

/// One `send_committee_message` attempt seen by the mock collector.
#[derive(Clone)]
pub(super) struct CapturedCommitteeSend {
    pub(super) message: UnsignedSSVMessage,
    pub(super) committee_id: CommitteeId,
    /// When the attempt was made.
    pub(super) sent_at: Instant,
    /// Whether the mock admitted the message rather than failing the attempt.
    pub(super) admitted: bool,
}

/// The mock collector's Boole committee selection half.
///
/// It records every `sign_committee_selection` and `send_committee_message` call apart from
/// [`CapturedCalls`], so the executions that publishing Boole assignments starts leave
/// `sign_and_collect` assertions untouched. Signing and sending succeed at once unless a test
/// fails, delays or holds them.
pub(super) struct CommitteeSelectionMock {
    signer: OperatorId,
    signed: Mutex<Vec<Arc<CommitteeSelectionBatch>>>,
    sends: Mutex<Vec<CapturedCommitteeSend>>,
    signing_failures_left: AtomicUsize,
    send_failures_left: AtomicUsize,
    signing_delay: Mutex<Duration>,
    /// `false` while signing is held.
    signing_released: watch::Sender<bool>,
}

impl CommitteeSelectionMock {
    fn new(signer: OperatorId) -> Self {
        Self {
            signer,
            signed: Mutex::new(Vec::new()),
            sends: Mutex::new(Vec::new()),
            signing_failures_left: AtomicUsize::new(0),
            send_failures_left: AtomicUsize::new(0),
            signing_delay: Mutex::new(Duration::ZERO),
            signing_released: watch::channel(true).0,
        }
    }

    /// Every batch handed to `sign_committee_selection`, in call order, including signings that
    /// failed or have not finished.
    pub(super) fn signed_batches(&self) -> Vec<Arc<CommitteeSelectionBatch>> {
        self.signed.lock().clone()
    }

    /// Every `send_committee_message` attempt, in call order.
    pub(super) fn sends(&self) -> Vec<CapturedCommitteeSend> {
        self.sends.lock().clone()
    }

    /// Fails the next `times` signings; `usize::MAX` fails every one.
    pub(super) fn fail_signing(&self, times: usize) {
        self.signing_failures_left.store(times, Ordering::SeqCst);
    }

    /// Fails the next `times` send attempts; `usize::MAX` fails every one.
    pub(super) fn fail_sends(&self, times: usize) {
        self.send_failures_left.store(times, Ordering::SeqCst);
    }

    /// Makes every later signing take `delay` to finish.
    pub(super) fn delay_signing(&self, delay: Duration) {
        *self.signing_delay.lock() = delay;
    }

    /// Makes signings wait, from their start, until [`Self::release_signing`].
    pub(super) fn hold_signing(&self) {
        self.signing_released.send_replace(false);
    }

    /// Lets held signings continue.
    pub(super) fn release_signing(&self) {
        self.signing_released.send_replace(true);
    }

    async fn sign(
        &self,
        batch: Arc<CommitteeSelectionBatch>,
    ) -> Result<UnsignedSSVMessage, CollectionError> {
        let ordinal = {
            let mut signed = self.signed.lock();
            signed.push(Arc::clone(&batch));
            signed.len()
        };
        let mut released = self.signing_released.subscribe();
        // The sender lives as long as `self`, which outlives this future.
        let _ = released.wait_for(|released| *released).await;
        let delay = *self.signing_delay.lock();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if take_scheduled_failure(&self.signing_failures_left) {
            return Err(CollectionError::QueueFullError);
        }
        Ok(mock_committee_selection_message(
            &batch,
            self.signer,
            ordinal,
        ))
    }

    fn send(
        &self,
        message: UnsignedSSVMessage,
        committee_id: CommitteeId,
    ) -> Result<(), SendError> {
        let admitted = !take_scheduled_failure(&self.send_failures_left);
        self.sends.lock().push(CapturedCommitteeSend {
            message,
            committee_id,
            sent_at: Instant::now(),
            admitted,
        });
        if admitted {
            Ok(())
        } else {
            Err(SendError::NotSynced)
        }
    }
}

/// Consumes one scheduled failure from `failures_left`, returning whether there was one.
fn take_scheduled_failure(failures_left: &AtomicUsize) -> bool {
    failures_left
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            left.checked_sub(1)
        })
        .is_ok()
}

/// The committee message the mock "signs" for `batch`: every entry in batch order with an empty
/// signature. `full_data` carries the signing's ordinal, so a message from a second signing never
/// equals the first even though real BLS signing is deterministic.
fn mock_committee_selection_message(
    batch: &CommitteeSelectionBatch,
    signer: OperatorId,
    ordinal: usize,
) -> UnsignedSSVMessage {
    let messages = batch
        .signing_data()
        .iter()
        .map(|entry| PartialSignatureMessage {
            partial_signature: Signature::empty(),
            signing_root: entry.root,
            signer,
            validator_index: entry.index,
        })
        .collect::<Vec<_>>();
    let messages = PartialSignatureMessages {
        kind: PartialSignatureKind::AggregatorCommitteePartialSig,
        slot: batch.slot(),
        messages: VariableList::new(messages).expect("a validated batch fits one message"),
    };
    UnsignedSSVMessage {
        ssv_message: SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            MessageId::new(
                &DomainType::default(),
                Role::AggregatorCommittee,
                &DutyExecutor::Committee(batch.committee_id()),
            ),
            messages.as_ssz_bytes(),
        )
        .expect("a validated batch fits one SSV message"),
        full_data: ordinal.to_le_bytes().to_vec(),
    }
}

// ==================== Committee setup ====================

pub(super) struct CommitteeSetup {
    pub(super) cluster: Cluster,
    pub(super) validators: Vec<ValidatorMetadata>,
    shares: Vec<Share>,
}

impl CommitteeSetup {
    /// Replaces validator `validator_idx`'s placeholder public key with `pubkey`, and stores every
    /// operator's share of it as `encrypted_share`. Only this operator's share is ever decrypted.
    pub(super) fn set_validator_key(
        &mut self,
        validator_idx: usize,
        pubkey: PublicKeyBytes,
        encrypted_share: [u8; ENCRYPTED_KEY_LENGTH],
    ) {
        let validator = &mut self.validators[validator_idx];
        for share in &mut self.shares {
            if share.validator_pubkey == validator.public_key {
                share.validator_pubkey = pubkey;
                share.encrypted_private_key = encrypted_share;
            }
        }
        validator.public_key = pubkey;
    }
}

/// Standard two-committee topology shared by the Boole+ callback-gate tests
/// (`committee_aggregate.rs` and `committee_contribution.rs`): a primary and a secondary
/// committee with distinct operator sets and disjoint validator index spaces, both containing
/// this operator.
pub(super) const PRIMARY_COMMITTEE_OPERATOR_IDS: [OperatorId; 4] =
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
pub(super) const SECONDARY_COMMITTEE_OPERATOR_IDS: [OperatorId; 4] =
    [OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)];
pub(super) const PRIMARY_COMMITTEE_STARTING_VALIDATOR_INDEX: usize = 0;
pub(super) const SECONDARY_COMMITTEE_STARTING_VALIDATOR_INDEX: usize = 100;

/// [`create_committee_setup`] for the standard primary committee.
pub(super) fn create_primary_committee_setup(num_validators: usize) -> CommitteeSetup {
    create_committee_setup(
        &PRIMARY_COMMITTEE_OPERATOR_IDS,
        num_validators,
        PRIMARY_COMMITTEE_STARTING_VALIDATOR_INDEX,
    )
}

/// [`create_committee_setup`] for the standard secondary committee.
pub(super) fn create_secondary_committee_setup(num_validators: usize) -> CommitteeSetup {
    create_committee_setup(
        &SECONDARY_COMMITTEE_OPERATOR_IDS,
        num_validators,
        SECONDARY_COMMITTEE_STARTING_VALIDATOR_INDEX,
    )
}

/// Asserts a callback stream yielded exactly one item and that item is an empty batch.
///
/// The empty batch is the Boole+ contract for both callback classes: Lighthouse's publish loops
/// drop an empty result silently, which is what keeps Anchor the single publisher of committee
/// duties. `what` names the class in failure messages ("aggregates", "sync contributions").
pub(super) fn assert_single_empty_batch<T>(results: Vec<Result<Vec<T>, Error>>, what: &str) {
    assert_eq!(results.len(), 1, "expected exactly one stream item");
    let batch = results
        .into_iter()
        .next()
        .expect("stream item should exist")
        .unwrap_or_else(|e| panic!("the callback for {what} should not fail: {e:?}"));
    assert!(
        batch.is_empty(),
        "Lighthouse must receive nothing to publish at Boole+; the metadata service publishes \
         committee {what} from the decided value"
    );
}

/// Builds a synthetic committee with deterministic validator and share data.
///
/// `starting_validator_index` lets tests create distinct committees without repeating the full
/// fixture setup inline.
pub(super) fn create_committee_setup(
    operator_ids: &[OperatorId],
    num_validators: usize,
    starting_validator_index: usize,
) -> CommitteeSetup {
    let cluster_id_bytes: [u8; 32] = rand::random();
    let cluster_id = ClusterId(cluster_id_bytes);

    let cluster = Cluster {
        cluster_id,
        owner: Default::default(),
        fee_recipient: Default::default(),
        liquidated: false,
        cluster_members: operator_ids.iter().copied().collect(),
    };

    let mut validators = Vec::with_capacity(num_validators);
    let mut shares = Vec::new();

    for i in 0..num_validators {
        let validator_pubkey =
            PublicKeyBytes::deserialize(&[(starting_validator_index + i) as u8; 48])
                .expect("valid length");

        let validator = ValidatorMetadata {
            public_key: validator_pubkey,
            cluster_id,
            index: Some(ValidatorIndex(starting_validator_index + i)),
            graffiti: Graffiti::default(),
        };

        for (j, &op_id) in operator_ids.iter().enumerate() {
            let mut share_bytes = [0u8; 48];
            share_bytes[0] = (starting_validator_index + i) as u8;
            share_bytes[1] = j as u8;

            shares.push(Share {
                validator_pubkey,
                operator_id: op_id,
                cluster_id,
                share_pubkey: PublicKeyBytes::deserialize(&share_bytes).expect("valid length"),
                encrypted_private_key: [0u8; ENCRYPTED_KEY_LENGTH],
            });
        }

        validators.push(validator);
    }

    CommitteeSetup {
        cluster,
        validators,
        shares,
    }
}

// ==================== Test harness ====================

pub(super) struct ValidatorStoreTestHarness {
    pub(super) validator_store:
        Arc<AnchorValidatorStore<ManualSlotClock, MainnetEthSpec, MockConsensusDecider>>,
    committee_setups: Vec<CommitteeSetup>,
    pub(super) captured_calls: CapturedCalls,
    /// Set by [`Self::fail_signature_collection`]; read by the mock collector on every call.
    signature_collection_fails: Arc<AtomicBool>,
    /// Filled by [`Self::hang_signature_collection`]; read by the mock collector on every call.
    signature_collection_hangs: Arc<AtomicBool>,
    /// Filled by [`Self::fail_signature_collection_for`]; read by the mock collector on every
    /// call.
    failing_pubkeys: FailingPubkeys,
    /// Records and steers the mock collector's Boole committee selection signing and sending.
    pub(super) committee_selection: Arc<CommitteeSelectionMock>,
    /// Shares `current_time` with the clone held by the store, so tests can reposition the clock
    /// after construction.
    pub(super) slot_clock: ManualSlotClock,
    pub(super) is_synced_tx: watch::Sender<bool>,
    _slashing_db_dir: TempDir,
    _exit_signal: async_channel::Sender<()>,
}

impl ValidatorStoreTestHarness {
    pub(super) fn new(committee_setups: Vec<CommitteeSetup>, our_operator_id: OperatorId) -> Self {
        Self::new_with_fork(committee_setups, our_operator_id, Fork::Boole)
    }

    pub(super) fn new_with_fork(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        active_fork: Fork,
    ) -> Self {
        // No proposer delay by default: most tests assert timing-free behaviour.
        Self::new_with_options(
            committee_setups,
            our_operator_id,
            active_fork,
            Duration::ZERO,
            MockConsensusDecider::default(),
        )
    }

    pub(super) fn new_with_proposer_delay(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        proposer_delay: Duration,
    ) -> Self {
        Self::new_with_options(
            committee_setups,
            our_operator_id,
            Fork::Boole,
            proposer_delay,
            MockConsensusDecider::default(),
        )
    }

    pub(super) fn new_with_fork_and_consensus(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        active_fork: Fork,
        consensus: MockConsensusDecider,
    ) -> Self {
        Self::new_with_options(
            committee_setups,
            our_operator_id,
            active_fork,
            Duration::ZERO,
            consensus,
        )
    }

    fn new_with_options(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        active_fork: Fork,
        proposer_delay: Duration,
        consensus: MockConsensusDecider,
    ) -> Self {
        // Dummy RSA key for database operator identification (not used for decryption)
        let rsa_pubkey = database::test_utils::generators::pubkey::random_rsa();

        // Slot clock positioned just past the 1/3 mark of TEST_SLOT
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            Duration::from_secs(0),
            Duration::from_secs(SLOT_DURATION_SECS),
        );
        let slot_start = TEST_SLOT * SLOT_DURATION_SECS;
        slot_clock.set_current_time(Duration::from_secs(
            slot_start + CLOCK_OFFSET_INTO_TEST_SLOT_SECS,
        ));

        let (executor, exit_signal) = create_test_executor();

        let fork_schedule = Arc::new(ForkSchedule::new(
            active_fork,
            ssv_types::domain_type::DomainType::default(),
            "test",
        ));

        let mock_collector = MockSignatureCollector::new(our_operator_id);

        // Database
        let database = Arc::new(
            NetworkDatabase::new_in_memory(&rsa_pubkey, "test")
                .expect("in-memory database should succeed"),
        );

        {
            let mut conn = database.connection().expect("connection should succeed");
            let tx = conn.transaction().expect("transaction should start");
            let mut pending = PendingStateUpdates::default();

            let mut inserted_operators = std::collections::HashSet::new();
            for setup in &committee_setups {
                for &op_id in &setup.cluster.cluster_members {
                    if inserted_operators.insert(op_id) {
                        let operator = if op_id == our_operator_id {
                            ssv_types::Operator::new_with_pubkey(
                                rsa_pubkey.clone(),
                                op_id,
                                types::Address::random(),
                            )
                        } else {
                            database::test_utils::generators::operator::with_id(op_id.0)
                        };
                        database
                            .insert_operator_tx(&operator, &tx, &mut pending)
                            .expect("operator insertion should succeed");
                    }
                }

                for validator in &setup.validators {
                    let validator_shares: Vec<Share> = setup
                        .shares
                        .iter()
                        .filter(|s| s.validator_pubkey == validator.public_key)
                        .cloned()
                        .collect();

                    database
                        .insert_validator_tx(
                            setup.cluster.clone(),
                            validator,
                            validator_shares,
                            &tx,
                            &mut pending,
                        )
                        .expect("validator insertion should succeed");
                }
            }

            tx.commit().expect("commit should succeed");
            database.publish_pending_state_updates(pending);
        }

        // Slashing DB
        let slashing_db_dir = TempDir::new().expect("tempdir should succeed");
        let slashing_protection = Arc::new(
            SlashingDatabase::open_or_create(&slashing_db_dir.path().join("slashing.sqlite"))
                .expect("slashing DB should succeed"),
        );

        let (is_synced_tx, is_synced_rx) = watch::channel(true);

        let validator_store = AnchorValidatorStore::new(
            database,
            Box::new(mock_collector.clone()),
            Arc::new(consensus),
            slashing_protection,
            true, // disable slashing protection for simpler testing
            slot_clock.clone(),
            Arc::new(ChainSpec::mainnet()),
            Hash256::zero(),
            None, // impostor mode: no RSA decryption needed
            fork_schedule,
            30_000_000,
            None,
            false,
            proposer_delay,
            false,
            is_synced_rx,
            executor,
        );

        Self {
            validator_store,
            committee_setups,
            captured_calls: mock_collector.captured,
            signature_collection_fails: mock_collector.fails,
            signature_collection_hangs: mock_collector.hangs,
            failing_pubkeys: mock_collector.failing_pubkeys,
            committee_selection: mock_collector.committee_selection,
            slot_clock,
            is_synced_tx,
            _slashing_db_dir: slashing_db_dir,
            _exit_signal: exit_signal,
        }
    }

    pub(super) fn validator_metadata(
        &self,
        committee_idx: usize,
        validator_idx: usize,
    ) -> ValidatorMetadata {
        self.committee_setups[committee_idx].validators[validator_idx].clone()
    }

    /// The beacon-chain validator index of one committee member, as it appears in signed
    /// aggregates and decided values.
    pub(super) fn aggregator_index(&self, committee_idx: usize, validator_idx: usize) -> u64 {
        *self
            .validator_metadata(committee_idx, validator_idx)
            .index
            .expect("test validator should have an index") as u64
    }

    pub(super) fn seed_sync_voting_assignments_for_slot(
        &self,
        slot: u64,
        assignments: Vec<(ValidatorIndex, Vec<(SyncSubnetId, usize)>)>,
    ) {
        let sync_validators_by_subnet = assignments
            .into_iter()
            .map(|(validator_index, position_counts)| {
                (validator_index, position_counts.into_iter().collect())
            })
            .collect();

        self.validator_store
            .update_voting_assignments(VotingAssignments {
                slot: Slot::new(slot),
                attesting_validators: Vec::new(),
                attesting_committees: HashMap::new(),
                sync_validators_by_subnet,
            });
    }

    /// Seeds the `VotingContext` so `get_voting_context` returns immediately for `TEST_SLOT`.
    pub(super) fn seed_voting_context(&self) {
        let mut attesting_committees = HashMap::new();
        let mut attesting_validators = Vec::new();

        for setup in &self.committee_setups {
            for (i, validator) in setup.validators.iter().enumerate() {
                if let Some(idx) = validator.index {
                    attesting_validators.push(idx);
                    attesting_committees.insert(validator.public_key, i as u64);
                }
            }
        }

        self.validator_store.update_voting_context(VotingContext {
            voting_assignments: Arc::new(VotingAssignments {
                slot: Slot::new(TEST_SLOT),
                attesting_validators,
                attesting_committees,
                sync_validators_by_subnet: HashMap::new(),
            }),
            beacon_vote: ssv_types::consensus::BeaconVote {
                block_root: Hash256::zero(),
                source: Checkpoint {
                    epoch: Epoch::new(0),
                    root: Hash256::zero(),
                },
                target: Checkpoint {
                    epoch: Epoch::new(0),
                    root: Hash256::zero(),
                },
            },
        });
    }

    /// Publishes `consensus_data_by_ssv_committee` as the decided values at `TEST_SLOT`, returning
    /// the executions the publish newly registered.
    ///
    /// This is the slot pipeline's Phase 3 step: registration happens before the assignments reach
    /// the watch channel, and only vacant registrations come back, so a republish returns nothing.
    pub(super) fn publish_decided_values(
        &self,
        consensus_data_by_ssv_committee: HashMap<
            CommitteeId,
            Arc<AggregatorCommitteeConsensusData<MainnetEthSpec>>,
        >,
    ) -> Vec<(CommitteeId, AggregatorPostConsensusShared<MainnetEthSpec>)> {
        self.validator_store
            .update_aggregation_assignments(AggregationAssignments {
                slot: Slot::new(TEST_SLOT),
                aggregator_committees: HashMap::new(),
                multi_sync_aggregators: HashMap::new(),
                consensus_data_by_ssv_committee,
            })
    }

    /// Runs the Lighthouse aggregate callback to completion.
    pub(super) async fn collect_aggregates(
        &self,
        aggregates: Vec<AggregateToSign<MainnetEthSpec>>,
    ) -> SignAggregatesResult {
        let stream = self.validator_store.sign_aggregate_and_proofs(aggregates);
        tokio::time::timeout(STREAM_TIMEOUT, stream.collect())
            .await
            .expect("the aggregate callback should complete within the stream timeout")
    }

    /// Runs the Lighthouse contribution callback to completion.
    pub(super) async fn collect_contributions(
        &self,
        contributions: Vec<ContributionToSign<MainnetEthSpec>>,
    ) -> SignContributionsResult {
        let stream = self
            .validator_store
            .sign_sync_committee_contributions(contributions);
        tokio::time::timeout(STREAM_TIMEOUT, stream.collect())
            .await
            .expect("the contribution callback should complete within the stream timeout")
    }

    /// A contribution request at `TEST_SLOT`, as Lighthouse's sync committee service would send.
    pub(super) fn create_contribution(
        &self,
        committee_idx: usize,
        validator_idx: usize,
        subcommittee_index: u64,
    ) -> ContributionToSign<MainnetEthSpec> {
        let validator = &self.committee_setups[committee_idx].validators[validator_idx];
        let validator_index = validator
            .index
            .expect("test validator should have an index");

        ContributionToSign {
            aggregator_index: *validator_index as u64,
            aggregator_pubkey: validator.public_key,
            contribution: SyncCommitteeContribution {
                slot: Slot::new(TEST_SLOT),
                beacon_block_root: Hash256::zero(),
                subcommittee_index,
                aggregation_bits: Default::default(),
                signature: AggregateSignature::infinity(),
            },
            selection_proof: SyncSelectionProof::from(Signature::empty()),
        }
    }

    /// Makes every subsequent `sign_and_collect` call fail, for tests of the paths a root that
    /// never reaches quorum takes. Calls are still captured.
    pub(super) fn fail_signature_collection(&self) {
        self.signature_collection_fails
            .store(true, Ordering::Relaxed);
    }

    /// Makes every subsequent `sign_and_collect` call hang forever, so the caller's own deadline
    /// decides each root's fate. This is the only way to reach a per-root deadline-expiry path;
    /// [`Self::fail_signature_collection`] resolves to an error instead. Calls are still captured.
    pub(super) fn hang_signature_collection(&self) {
        self.signature_collection_hangs
            .store(true, Ordering::Relaxed);
    }

    /// Makes every subsequent `sign_and_collect` call for `pubkey` fail, so one root can miss
    /// quorum while its siblings still collect. Calls are still captured.
    pub(super) fn fail_signature_collection_for(&self, pubkey: PublicKeyBytes) {
        self.failing_pubkeys.lock().insert(pubkey);
    }

    pub(super) fn create_attestation(
        &self,
        committee_idx: usize,
        validator_idx: usize,
    ) -> AttestationToSign<MainnetEthSpec> {
        self.create_attestation_at_slot(committee_idx, validator_idx, TEST_SLOT)
    }

    pub(super) fn create_attestation_at_slot(
        &self,
        committee_idx: usize,
        validator_idx: usize,
        slot: u64,
    ) -> AttestationToSign<MainnetEthSpec> {
        let validator = &self.committee_setups[committee_idx].validators[validator_idx];
        let validator_index = validator
            .index
            .expect("test validator should have an index");

        AttestationToSign {
            validator_index: *validator_index as u64,
            pubkey: validator.public_key,
            validator_committee_index: 0,
            attestation: Attestation::Base(AttestationBase {
                aggregation_bits: ssz_types::BitList::with_capacity(128)
                    .expect("bitlist should be valid"),
                data: AttestationData {
                    slot: Slot::new(slot),
                    index: 0,
                    beacon_block_root: Hash256::zero(),
                    source: Checkpoint {
                        epoch: Epoch::new(0),
                        root: Hash256::zero(),
                    },
                    target: Checkpoint {
                        epoch: Epoch::new(0),
                        root: Hash256::zero(),
                    },
                },
                signature: AggregateSignature::infinity(),
            }),
        }
    }

    pub(super) fn create_aggregate(
        &self,
        committee_idx: usize,
        validator_idx: usize,
    ) -> AggregateToSign<MainnetEthSpec> {
        let validator = &self.committee_setups[committee_idx].validators[validator_idx];
        let validator_index = validator
            .index
            .expect("test validator should have an index");

        AggregateToSign {
            pubkey: validator.public_key,
            aggregator_index: *validator_index as u64,
            aggregate: Attestation::Base(AttestationBase {
                aggregation_bits: ssz_types::BitList::with_capacity(128)
                    .expect("bitlist should be valid"),
                data: AttestationData {
                    slot: Slot::new(TEST_SLOT),
                    index: committee_idx as u64,
                    beacon_block_root: Hash256::zero(),
                    source: Checkpoint {
                        epoch: Epoch::new(0),
                        root: Hash256::zero(),
                    },
                    target: Checkpoint {
                        epoch: Epoch::new(0),
                        root: Hash256::zero(),
                    },
                },
                signature: AggregateSignature::infinity(),
            }),
            selection_proof: SelectionProof::from(Signature::empty()),
        }
    }
}

fn create_test_executor() -> (TaskExecutor, async_channel::Sender<()>) {
    let handle = tokio::runtime::Handle::current();
    let (signal, exit) = async_channel::bounded::<()>(1);
    let (shutdown, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown);
    (executor, signal)
}
