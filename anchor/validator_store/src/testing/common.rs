//! Shared test infrastructure for `AnchorValidatorStore` integration tests.
//!
//! Provides a `ValidatorStoreTestHarness` that wires up a real `AnchorValidatorStore` with
//! in-memory database, mock consensus, and a mock signature collector.

use std::{
    any::Any,
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bls::{AggregateSignature, FixedBytesExtended, PublicKeyBytes, Signature};
use database::{NetworkDatabase, PendingStateUpdates};
use dissemination_store::DisseminationStore;
use fork::{Fork, ForkSchedule};
use futures::StreamExt;
use parking_lot::Mutex;
use qbft::Completed;
use qbft_manager::{ConsensusDecider, QbftDecidable, QbftError, TimeoutMode};
use signature_collector::{
    CollectionError, SignatureCollecting, SignatureMetadata, SignatureRequester,
    ValidatorSigningData,
};
use slashing_protection::SlashingDatabase;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    Cluster, ClusterId, CommitteeId, ENCRYPTED_KEY_LENGTH, IndexSet, OperatorId, Share,
    ValidatorIndex, ValidatorMetadata,
    consensus::{AggregatorCommitteeConsensusData, BeaconVote, GloasBeaconVote, QbftDataValidator},
    dissemination::EnvelopeDissemination,
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
    SingleAttestation, Slot, SyncCommitteeContribution, SyncSelectionProof, SyncSubnetId,
};
use validator_store::{
    AggregateToSign, AttestationToSign, ContributionToSign, SyncMessageToSign, ValidatorStore,
};

use crate::{
    AggregationAssignments, AnchorValidatorStore, Error, ProposerDelays, VotingAssignments,
    VotingContext, aggregator_post_consensus::AggregatorPostConsensusShared,
};

pub(super) const TEST_SLOT: u64 = 1;
pub(super) const SLOT_DURATION_SECS: u64 = 12;
/// How far into `TEST_SLOT` the harness slot clock sits (just past the 1/3 mark).
pub(super) const CLOCK_OFFSET_INTO_TEST_SLOT_SECS: u64 = SLOT_DURATION_SECS / 3 + 1;

/// The raw item stream `sign_attestations` yields: one `Result` batch per committee.
pub(super) type SignAttestationsResult = Vec<Result<Vec<SingleAttestation>, Error>>;

/// Bound on any Lighthouse callback stream in these tests; a callback that blocks past it is a
/// failure, not a slow machine.
pub(super) const STREAM_TIMEOUT: Duration = Duration::from_secs(5);

/// What the Lighthouse aggregate callback yields: one result per stream item.
pub(super) type SignAggregatesResult =
    Vec<Result<Vec<SignedAggregateAndProof<MainnetEthSpec>>, Error>>;

/// What the Lighthouse contribution callback yields: one result per stream item.
pub(super) type SignContributionsResult =
    Vec<Result<Vec<SignedContributionAndProof<MainnetEthSpec>>, Error>>;

/// Drives `sign_attestations` to completion and unwraps every committee batch.
pub(super) async fn run_sign_attestations(
    harness: &ValidatorStoreTestHarness,
    attestations: Vec<AttestationToSign>,
) -> Vec<SingleAttestation> {
    let results: SignAttestationsResult = harness
        .validator_store
        .sign_attestations(attestations)
        .collect()
        .await;
    results
        .into_iter()
        .flat_map(|batch| batch.expect("committee batch should succeed"))
        .collect()
}

// ==================== Mock consensus decider ====================

/// Mock that instantly returns `Completed::Success(initial)`, echoing back the proposed data.
/// Removes the need for `QbftManager` infrastructure and lets the signing pipeline run fully.
///
/// When `forced_gloas_index` is `Some`, a `GloasBeaconVote` seed is decided with its
/// `attestation_data_index` overridden to that value (all other fields preserved). This lets
/// tests exercise "the cluster-decided index differs from this operator's local seed", which is
/// exactly the case `#1027` must apply. Non-Gloas seeds (`BeaconVote`) are always echoed back
/// unchanged, since their decided value carries no index.
///
/// When `fixed_decision` is `Some`, every seed decides as that SSZ-encoded value once `parties`
/// callers have reached the barrier, modeling a cluster decision that differs from each caller's
/// own proposal.
#[derive(Default)]
pub(super) struct MockConsensusDecider {
    forced_gloas_index: Option<u64>,
    fixed_decision: Option<(Vec<u8>, Arc<Barrier>)>,
    captured_timeouts: Arc<Mutex<Vec<TimeoutMode>>>,
}

impl MockConsensusDecider {
    /// Echoes every decided seed back unchanged (default behavior used by most tests).
    pub(super) fn echoing() -> Self {
        Self::default()
    }

    /// Decides every seed as `value` once `parties` callers have reached the barrier.
    pub(super) fn fixed_after_barrier<D: Encode>(value: &D, parties: usize) -> Self {
        Self {
            fixed_decision: Some((value.as_ssz_bytes(), Arc::new(Barrier::new(parties)))),
            ..Self::default()
        }
    }

    /// Decides every `GloasBeaconVote` seed with its `attestation_data_index` replaced by
    /// `index`, modeling a cluster that agrees on an index that may differ from the local seed.
    pub(super) fn forcing_gloas_index(index: u64) -> Self {
        Self {
            forced_gloas_index: Some(index),
            ..Self::default()
        }
    }
}

impl<E: EthSpec> ConsensusDecider<E> for MockConsensusDecider {
    async fn decide_instance<D: QbftDecidable<E>>(
        &self,
        _id: D::Id,
        initial: D,
        _validator: Box<dyn QbftDataValidator<D>>,
        timeout_mode: TimeoutMode,
        _committee_members: &IndexSet<OperatorId>,
    ) -> Result<Completed<D>, QbftError> {
        self.captured_timeouts.lock().push(timeout_mode);
        // `D: QbftDecidable<E>` requires `'static`, so this downcast is sound. Only the Gloas
        // seed type carries `attestation_data_index`; every other `D` falls through to the echo.
        if let Some(index) = self.forced_gloas_index {
            let boxed: Box<dyn Any> = Box::new(initial);
            match boxed.downcast::<GloasBeaconVote>() {
                Ok(gloas_vote) => {
                    let decided = GloasBeaconVote {
                        attestation_data_index: index,
                        ..*gloas_vote
                    };
                    // Re-box and downcast back to `D`; the inner type is `GloasBeaconVote == D`
                    // here, so this recovers the concrete decided value the caller expects.
                    let decided_any: Box<dyn Any> = Box::new(decided);
                    let decided_d = decided_any
                        .downcast::<D>()
                        .expect("GloasBeaconVote round-trips to D when D == GloasBeaconVote");
                    return Ok(Completed::Success(*decided_d));
                }
                Err(original) => {
                    // Not a Gloas seed; recover the original value and echo it back unchanged.
                    let echoed = original
                        .downcast::<D>()
                        .expect("downcast back to original D always succeeds");
                    return Ok(Completed::Success(*echoed));
                }
            }
        }
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

/// One captured `broadcast_dissemination` call.
#[derive(Debug, Clone)]
pub(super) struct CapturedDissemination {
    pub(super) validator_pubkey: PublicKeyBytes,
    pub(super) committee_id: CommitteeId,
    pub(super) dissemination: EnvelopeDissemination,
}

/// Shared storage for captured `broadcast_dissemination` calls.
pub(super) type CapturedDisseminations = Arc<Mutex<Vec<CapturedDissemination>>>;

/// Shared storage for captured `sign_and_collect` calls.
pub(super) type CapturedCalls = Arc<Mutex<Vec<CapturedSignatureCall>>>;

pub(super) struct CapturedSignatureCall {
    pub(super) requester: SignatureRequester,
    pub(super) metadata: SignatureMetadata,
    /// Only the root is captured, not the full `ValidatorSigningData`, so the capture never
    /// holds key share material.
    pub(super) signing_root: Hash256,
    pub(super) validator_pubkey: PublicKeyBytes,
    /// When the call was made.
    pub(super) captured_at: Instant,
}

/// Validator pubkeys whose signature collection fails; shared with the harness so tests can fail
/// a single validator's roots.
type FailingPubkeys = Arc<Mutex<HashSet<PublicKeyBytes>>>;

/// Mock that captures calls and returns a canned infinity signature, or one of the configured
/// failure modes: the `HarnessOptions` error for every call, a collection timeout for every call
/// once [`ValidatorStoreTestHarness::fail_signature_collection`] is called, a timeout for one
/// validator's calls once [`ValidatorStoreTestHarness::fail_signature_collection_for`] is, or a
/// future that never resolves (`hangs`).
///
/// Failing and hanging are different: a failure resolves to an error, which readers count as
/// `other_error`, while a hang leaves the root pending so the reader's own deadline decides its
/// fate. Only the hang mode reaches a per-root deadline-expiry path. Hanging takes precedence
/// over both failure modes.
struct MockSignatureCollector {
    captured: CapturedCalls,
    captured_disseminations: CapturedDisseminations,
    /// Set from `HarnessOptions::dissemination_failure`; every broadcast fails with this error.
    dissemination_failure: Option<CollectionError>,
    /// Set from `HarnessOptions::collector_failure`; every call fails with this error.
    failure: Option<CollectionError>,
    fails: Arc<AtomicBool>,
    /// Starts from `HarnessOptions::collector_hangs`; also set by
    /// [`ValidatorStoreTestHarness::hang_signature_collection`].
    hangs: Arc<AtomicBool>,
    failing_pubkeys: FailingPubkeys,
}

impl SignatureCollecting for MockSignatureCollector {
    fn sign_and_collect(
        &self,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        signing_data: ValidatorSigningData,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<Signature>, CollectionError>> + Send + '_>> {
        // Capture before hanging/failing so tests can assert that the collection attempt happened
        // even when the configured outcome is a stall or an error.
        self.captured.lock().push(CapturedSignatureCall {
            requester,
            metadata,
            signing_root: signing_data.root,
            validator_pubkey: signing_data.validator_pubkey,
            captured_at: Instant::now(),
        });
        // Stands in for a root whose quorum never arrives: the future stays pending, so the
        // caller's deadline is what ends the wait.
        // `std::future::pending::<Result<Arc<Signature>, CollectionError>>()` is `Send`, which
        // satisfies the returned future's bound.
        if self.hangs.load(Ordering::Relaxed) {
            return Box::pin(std::future::pending());
        }
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
        if let Some(failure) = self.failure.clone() {
            return Box::pin(async move { Err(failure) });
        }
        let sig = Signature::infinity().expect("infinity signature");
        Box::pin(async move { Ok(Arc::new(sig)) })
    }

    fn broadcast_dissemination(
        &self,
        validator_pubkey: PublicKeyBytes,
        committee_id: CommitteeId,
        dissemination: EnvelopeDissemination,
    ) -> Result<(), CollectionError> {
        if let Some(failure) = self.dissemination_failure.clone() {
            return Err(failure);
        }
        self.captured_disseminations
            .lock()
            .push(CapturedDissemination {
                validator_pubkey,
                committee_id,
                dissemination,
            });
        Ok(())
    }
}

/// Creates a mock signature collector, returning the shared captured calls and failure-mode
/// handles.
fn create_mock_collector(
    failure: Option<CollectionError>,
    dissemination_failure: Option<CollectionError>,
    hang: bool,
) -> (
    Box<dyn SignatureCollecting>,
    CapturedCalls,
    CapturedDisseminations,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    FailingPubkeys,
) {
    let captured: CapturedCalls = Arc::new(Mutex::new(Vec::new()));
    let captured_disseminations: CapturedDisseminations = Arc::new(Mutex::new(Vec::new()));
    let fails = Arc::new(AtomicBool::new(false));
    let hangs = Arc::new(AtomicBool::new(hang));
    let failing_pubkeys: FailingPubkeys = Arc::new(Mutex::new(HashSet::new()));
    let mock = MockSignatureCollector {
        captured: Arc::clone(&captured),
        captured_disseminations: Arc::clone(&captured_disseminations),
        dissemination_failure,
        failure,
        fails: Arc::clone(&fails),
        hangs: Arc::clone(&hangs),
        failing_pubkeys: Arc::clone(&failing_pubkeys),
    };
    (
        Box::new(mock),
        captured,
        captured_disseminations,
        fails,
        hangs,
        failing_pubkeys,
    )
}

// ==================== Committee setup ====================

pub(super) struct CommitteeSetup {
    pub(super) cluster: Cluster,
    pub(super) validators: Vec<ValidatorMetadata>,
    shares: Vec<Share>,
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

/// Construction knobs that only some tests need to vary.
pub(super) struct HarnessOptions {
    /// When set, every `sign_and_collect` call fails with this error after being captured.
    pub(super) collector_failure: Option<CollectionError>,
    /// Every `broadcast_dissemination` call fails with this error.
    pub(super) dissemination_failure: Option<CollectionError>,
    /// When `true`, every `sign_and_collect` call captures the call and then returns a future that
    /// never resolves, modeling a quorum that never forms. Used to drive the production
    /// collection-timeout path. Takes precedence over `collector_failure`.
    pub(super) collector_hangs: bool,
    pub(super) disable_slashing_protection: bool,
    /// Chain spec to wire into the store. Defaults to `ChainSpec::mainnet()`, under which
    /// `TEST_SLOT` is pre-Electra. Tests that need a specific fork at `TEST_SLOT` (Electra or
    /// Gloas) supply a spec with the matching `*_fork_epoch` set to genesis.
    pub(super) spec: Arc<ChainSpec>,
    /// Consensus decider wired into the store. Defaults to the echoing mock; tests that need a
    /// cluster decision differing from the local seed supply a configured
    /// [`MockConsensusDecider`].
    pub(super) decider: MockConsensusDecider,
    /// SSV fork the store's `ForkSchedule` reports as active. Defaults to `Boole`; tests that
    /// exercise pre-Boole behaviour supply an earlier fork.
    pub(super) active_fork: Fork,
    /// Proposer delays wired into the store. Both default to zero so most tests assert
    /// timing-free behaviour.
    pub(super) proposer_delays: ProposerDelays,
}

impl Default for HarnessOptions {
    fn default() -> Self {
        Self {
            collector_failure: None,
            dissemination_failure: None,
            collector_hangs: false,
            // Slashing protection is disabled by default; most tests do not exercise it. When a
            // test enables it, the harness registers every configured validator in the slashing
            // DB so the signing paths can record and check attestations.
            disable_slashing_protection: true,
            spec: Arc::new(ChainSpec::mainnet()),
            decider: MockConsensusDecider::echoing(),
            active_fork: Fork::Boole,
            proposer_delays: ProposerDelays::default(),
        }
    }
}

/// Builds a `ChainSpec` based on mainnet but with Gloas activated at genesis, so `TEST_SLOT`
/// resolves to the Gloas fork and the committee QBFT decides over `GloasBeaconVote`.
pub(super) fn gloas_at_genesis_spec() -> Arc<ChainSpec> {
    let mut spec = ChainSpec::mainnet();
    spec.gloas_fork_epoch = Some(Epoch::new(0));
    Arc::new(spec)
}

/// Builds a `ChainSpec` based on mainnet but with Gloas activated at `gloas_epoch` (leaving
/// mainnet's other fork epochs untouched, so they stay far in the future). This places a fork
/// boundary strictly inside a lookahead window: an epoch below `gloas_epoch` resolves to the
/// genesis fork version while `gloas_epoch` and beyond resolve to the Gloas fork version, giving
/// two byte-distinct signing domains on either side of the boundary. Used to make the
/// "domain keyed on the *proposal* epoch, not the send epoch" assertion falsifiable.
pub(super) fn gloas_at_epoch_spec(gloas_epoch: Epoch) -> Arc<ChainSpec> {
    let mut spec = ChainSpec::mainnet();
    spec.gloas_fork_epoch = Some(gloas_epoch);
    Arc::new(spec)
}

/// Builds a `ChainSpec` based on mainnet but with Electra activated at genesis and Gloas
/// disabled, so `TEST_SLOT` resolves to Electra (post-Electra, pre-Gloas). The committee QBFT
/// decides over `BeaconVote` and the attestation index stays untouched at `0`.
pub(super) fn electra_at_genesis_spec() -> Arc<ChainSpec> {
    let mut spec = ChainSpec::mainnet();
    spec.electra_fork_epoch = Some(Epoch::new(0));
    spec.gloas_fork_epoch = None;
    Arc::new(spec)
}

pub(super) struct ValidatorStoreTestHarness {
    pub(super) validator_store:
        Arc<AnchorValidatorStore<ManualSlotClock, MainnetEthSpec, MockConsensusDecider>>,
    committee_setups: Vec<CommitteeSetup>,
    pub(super) captured_calls: CapturedCalls,
    pub(super) captured_disseminations: CapturedDisseminations,
    /// Timeout origins supplied by the real signing and committee consensus callers.
    pub(super) captured_consensus_timeouts: Arc<Mutex<Vec<TimeoutMode>>>,
    /// The dissemination handoff store the store awaits on; tests insert into it to stand in
    /// for the message receiver.
    pub(super) dissemination_store: Arc<DisseminationStore>,
    /// Set by [`Self::fail_signature_collection`]; read by the mock collector on every call.
    signature_collection_fails: Arc<AtomicBool>,
    /// Filled by [`Self::hang_signature_collection`]; read by the mock collector on every call.
    signature_collection_hangs: Arc<AtomicBool>,
    /// Filled by [`Self::fail_signature_collection_for`]; read by the mock collector on every
    /// call.
    failing_pubkeys: FailingPubkeys,
    /// Shares `current_time` with the clone held by the store, so tests can reposition the clock
    /// after construction.
    pub(super) slot_clock: ManualSlotClock,
    pub(super) is_synced_tx: watch::Sender<bool>,
    /// The spec the store was built with, exposed so tests can recompute signing domains
    /// without duplicating the store's fork-selection logic.
    pub(super) spec: Arc<ChainSpec>,
    /// Genesis validators root the store was built with (`Hash256::zero()`), needed alongside
    /// `spec` to recompute signing roots.
    pub(super) genesis_validators_root: Hash256,
    /// The slashing DB the store writes to, exposed so tests that enable slashing protection can
    /// probe what the production path recorded.
    pub(super) slashing_protection: Arc<SlashingDatabase>,
    _slashing_db_dir: TempDir,
    _exit_signal: async_channel::Sender<()>,
}

impl ValidatorStoreTestHarness {
    pub(super) fn new(committee_setups: Vec<CommitteeSetup>, our_operator_id: OperatorId) -> Self {
        Self::new_with_options(committee_setups, our_operator_id, HarnessOptions::default())
    }

    pub(super) fn new_with_fork(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        active_fork: Fork,
    ) -> Self {
        Self::new_with_options(
            committee_setups,
            our_operator_id,
            HarnessOptions {
                active_fork,
                ..Default::default()
            },
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
            HarnessOptions {
                active_fork,
                decider: consensus,
                ..Default::default()
            },
        )
    }

    pub(super) fn new_with_options(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        options: HarnessOptions,
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
            options.active_fork,
            ssv_types::domain_type::DomainType::default(),
            "test",
        ));

        let (
            mock_collector,
            captured_calls,
            captured_disseminations,
            signature_collection_fails,
            signature_collection_hangs,
            failing_pubkeys,
        ) = create_mock_collector(
            options.collector_failure,
            options.dissemination_failure,
            options.collector_hangs,
        );

        let dissemination_store = Arc::new(DisseminationStore::new());

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

        // When slashing protection is enabled, register every validator so the signing paths can
        // record and check attestations instead of failing with `UnregisteredValidator`.
        if !options.disable_slashing_protection {
            slashing_protection
                .register_validators(
                    committee_setups
                        .iter()
                        .flat_map(|setup| setup.validators.iter().map(|v| &v.public_key)),
                )
                .expect("validator registration should succeed");
        }

        let (is_synced_tx, is_synced_rx) = watch::channel(true);

        let decider = options.decider;
        let captured_consensus_timeouts = Arc::clone(&decider.captured_timeouts);

        let spec = Arc::clone(&options.spec);
        let genesis_validators_root = Hash256::zero();

        let validator_store = AnchorValidatorStore::new(
            database,
            mock_collector,
            Arc::clone(&dissemination_store),
            Arc::new(decider),
            Arc::clone(&slashing_protection),
            options.disable_slashing_protection,
            slot_clock.clone(),
            Arc::clone(&spec),
            genesis_validators_root,
            None, // impostor mode: no RSA decryption needed
            fork_schedule,
            30_000_000,
            None,
            false,
            options.proposer_delays,
            false,
            is_synced_rx,
            executor,
        );

        Self {
            validator_store,
            committee_setups,
            captured_calls,
            captured_disseminations,
            captured_consensus_timeouts,
            dissemination_store,
            signature_collection_fails,
            signature_collection_hangs,
            failing_pubkeys,
            slot_clock,
            is_synced_tx,
            spec,
            genesis_validators_root,
            slashing_protection,
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

    /// Builds the `VotingAssignments` for `TEST_SLOT`, marking every validator in every committee
    /// setup as attesting. Shared by the Base and Gloas seeders so they only differ in the vote.
    fn test_slot_voting_assignments(&self) -> VotingAssignments {
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

        VotingAssignments {
            slot: Slot::new(TEST_SLOT),
            attesting_validators,
            attesting_committees,
            sync_validators_by_subnet: HashMap::new(),
        }
    }

    pub(super) fn zero_checkpoint() -> Checkpoint {
        Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::zero(),
        }
    }

    /// Seeds the `VotingContext` with a pre-Gloas `BeaconVote` so `get_voting_context` returns
    /// immediately for `TEST_SLOT`. Used by harnesses on a pre-Gloas spec (Base path).
    pub(super) fn seed_voting_context(&self) {
        self.seed_base_voting_context_with_vote(BeaconVote {
            block_root: Hash256::zero(),
            source: Self::zero_checkpoint(),
            target: Self::zero_checkpoint(),
        });
    }

    /// Seeds the pre-Gloas voting context with an explicit vote, allowing tests to make the
    /// shared metadata-service seed differ from the incoming attestation duty.
    pub(super) fn seed_base_voting_context_with_vote(&self, vote: BeaconVote) {
        self.validator_store.update_voting_context(VotingContext {
            voting_assignments: Arc::new(self.test_slot_voting_assignments()),
            vote: crate::SlotVote::Base(vote),
            same_slot_head_root: None,
            decided_votes: Default::default(),
        });
    }

    /// Seeds the `VotingContext` with a Gloas `GloasBeaconVote` carrying `attestation_data_index`,
    /// modeling this operator's local seed for the committee QBFT. Used by harnesses on a
    /// Gloas-enabled spec (Gloas path).
    pub(super) fn seed_gloas_voting_context(&self, attestation_data_index: u64) {
        self.seed_gloas_voting_context_with_vote(GloasBeaconVote {
            block_root: Hash256::zero(),
            source: Self::zero_checkpoint(),
            target: Self::zero_checkpoint(),
            attestation_data_index,
        });
    }

    /// Seeds the Gloas voting context with an explicit vote, allowing tests to distinguish the
    /// shared metadata-service seed from the incoming attestation duty.
    pub(super) fn seed_gloas_voting_context_with_vote(&self, vote: GloasBeaconVote) {
        self.seed_gloas_voting_context_with_head(vote, None);
    }

    /// Like `seed_gloas_voting_context_with_vote`, also recording the head root a same-slot head
    /// event would have fixed for `TEST_SLOT`.
    pub(super) fn seed_gloas_voting_context_with_head(
        &self,
        vote: GloasBeaconVote,
        same_slot_head_root: Option<Hash256>,
    ) {
        self.validator_store.update_voting_context(VotingContext {
            voting_assignments: Arc::new(self.test_slot_voting_assignments()),
            vote: crate::SlotVote::Gloas(vote),
            same_slot_head_root,
            decided_votes: Default::default(),
        });
    }

    /// Reads the slot-local committee decision recorded by the production voting path.
    pub(super) async fn cached_vote_for_committee(
        &self,
        committee_id: CommitteeId,
    ) -> Option<crate::SlotVote> {
        let voting_context = self
            .validator_store
            .get_voting_context(Slot::new(TEST_SLOT))
            .await
            .expect("test voting context should be available");
        voting_context
            .decided_votes
            .lock()
            .get(&committee_id)
            .cloned()
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
    ) -> AttestationToSign {
        self.create_attestation_at_slot(committee_idx, validator_idx, TEST_SLOT)
    }

    pub(super) fn create_attestation_at_slot(
        &self,
        committee_idx: usize,
        validator_idx: usize,
        slot: u64,
    ) -> AttestationToSign {
        let validator = &self.committee_setups[committee_idx].validators[validator_idx];
        let validator_index = validator
            .index
            .expect("test validator should have an index");

        AttestationToSign {
            attester_index: *validator_index as u64,
            pubkey: validator.public_key,
            // Matches the `attesting_committees` entry seeded by
            // `test_slot_voting_assignments` (the validator's position within its committee),
            // so the duty passes the production identity check by default.
            committee_index: validator_idx as u64,
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
        }
    }

    /// The public key of validator `validator_idx` in committee `committee_idx`.
    pub(super) fn validator_pubkey(
        &self,
        committee_idx: usize,
        validator_idx: usize,
    ) -> PublicKeyBytes {
        self.committee_setups[committee_idx].validators[validator_idx].public_key
    }

    /// Builds an attestation duty for `TEST_SLOT` whose `data.index` is pre-set to `index`,
    /// modeling a BN-supplied attestation index (e.g. the committee index pre-Electra). Lets
    /// tests assert whether the signing path leaves that index untouched or overwrites it.
    pub(super) fn create_attestation_with_index(
        &self,
        committee_idx: usize,
        validator_idx: usize,
        index: u64,
    ) -> AttestationToSign {
        let mut att = self.create_attestation(committee_idx, validator_idx);
        att.data.index = index;
        att
    }

    /// Builds a sync-committee message duty for `TEST_SLOT`, so the sync signing path can be
    /// driven over the same `(committee, slot)` as the attestation path.
    pub(super) fn create_sync_message(
        &self,
        committee_idx: usize,
        validator_idx: usize,
    ) -> SyncMessageToSign {
        let validator = &self.committee_setups[committee_idx].validators[validator_idx];
        let validator_index = validator
            .index
            .expect("test validator should have an index");

        SyncMessageToSign {
            slot: Slot::new(TEST_SLOT),
            beacon_block_root: Hash256::zero(),
            validator_index: *validator_index as u64,
            pubkey: validator.public_key,
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
