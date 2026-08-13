//! Shared test infrastructure for `AnchorValidatorStore` integration tests.
//!
//! Provides a `ValidatorStoreTestHarness` that wires up a real `AnchorValidatorStore` with
//! in-memory database, mock consensus, and a mock signature collector.

use std::{
    collections::HashMap,
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
    consensus::{AggregatorCommitteeConsensusData, QbftDataValidator},
};
use task_executor::TaskExecutor;
use tempfile::TempDir;
use tokio::{sync::watch, time::Instant};
use types::{
    Attestation, AttestationBase, AttestationData, ChainSpec, Checkpoint, Epoch, EthSpec, Graffiti,
    Hash256, MainnetEthSpec, SelectionProof, SignedAggregateAndProof, Slot, SyncSubnetId,
};
use validator_store::{AggregateToSign, AttestationToSign, ValidatorStore};

use crate::{
    AggregationAssignments, AnchorValidatorStore, Error, VotingAssignments, VotingContext,
    aggregator_post_consensus::AggregatorPostConsensusShared,
};

pub(super) const TEST_SLOT: u64 = 1;
pub(super) const SLOT_DURATION_SECS: u64 = 12;
/// Bound on any Lighthouse callback stream in these tests; a callback that blocks past it is a
/// failure, not a slow machine.
pub(super) const STREAM_TIMEOUT: Duration = Duration::from_secs(5);

/// What the Lighthouse aggregate callback yields: one result per stream item.
pub(super) type SignAggregatesResult =
    Vec<Result<Vec<SignedAggregateAndProof<MainnetEthSpec>>, Error>>;

// ==================== Mock consensus decider ====================

/// Mock that instantly returns `Completed::Success(initial)`, echoing back the proposed data.
/// Removes the need for `QbftManager` infrastructure and lets the signing pipeline run fully.
pub(super) struct MockConsensusDecider;

impl<E: EthSpec> ConsensusDecider<E> for MockConsensusDecider {
    async fn decide_instance<D: QbftDecidable<E>>(
        &self,
        _id: D::Id,
        initial: D,
        _validator: Box<dyn QbftDataValidator<D>>,
        _timeout_mode: TimeoutMode,
        _committee_members: &IndexSet<OperatorId>,
    ) -> Result<Completed<D>, QbftError> {
        Ok(Completed::Success(initial))
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

/// Mock that captures calls and returns a canned infinity signature, or a collection timeout once
/// [`ValidatorStoreTestHarness::fail_signature_collection`] is called.
struct MockSignatureCollector {
    captured: CapturedCalls,
    fails: Arc<AtomicBool>,
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
        if self.fails.load(Ordering::Relaxed) {
            return Box::pin(async { Err(CollectionError::CollectionTimeout) });
        }
        let sig = Signature::infinity().expect("infinity signature");
        Box::pin(async move { Ok(Arc::new(sig)) })
    }
}

/// Creates a mock signature collector, returning the shared captured calls and failure-mode
/// handles.
fn create_mock_collector() -> (Box<dyn SignatureCollecting>, CapturedCalls, Arc<AtomicBool>) {
    let captured: CapturedCalls = Arc::new(Mutex::new(Vec::new()));
    let fails = Arc::new(AtomicBool::new(false));
    let mock = MockSignatureCollector {
        captured: Arc::clone(&captured),
        fails: Arc::clone(&fails),
    };
    (Box::new(mock), captured, fails)
}

// ==================== Committee setup ====================

pub(super) struct CommitteeSetup {
    pub(super) cluster: Cluster,
    pub(super) validators: Vec<ValidatorMetadata>,
    shares: Vec<Share>,
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
        )
    }

    fn new_with_options(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        active_fork: Fork,
        proposer_delay: Duration,
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
        slot_clock.set_current_time(Duration::from_secs(slot_start + SLOT_DURATION_SECS / 3 + 1));

        let (executor, exit_signal) = create_test_executor();

        let fork_schedule = Arc::new(ForkSchedule::new(
            active_fork,
            ssv_types::domain_type::DomainType::default(),
            "test",
        ));

        let (mock_collector, captured_calls, signature_collection_fails) = create_mock_collector();

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
            mock_collector,
            Arc::new(MockConsensusDecider),
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
            captured_calls,
            signature_collection_fails,
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

    /// Makes every subsequent `sign_and_collect` call fail, for tests of the paths a root that
    /// never reaches quorum takes. Calls are still captured.
    pub(super) fn fail_signature_collection(&self) {
        self.signature_collection_fails
            .store(true, Ordering::Relaxed);
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
