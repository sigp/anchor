//! Shared test infrastructure for `AnchorValidatorStore` integration tests.
//!
//! Provides a `ValidatorStoreTestHarness` that wires up a real `AnchorValidatorStore` with
//! in-memory database, mock consensus, and a mock signature collector.

use std::{any::Any, collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};

use bls::{AggregateSignature, FixedBytesExtended, PublicKeyBytes, Signature};
use database::{NetworkDatabase, PendingStateUpdates};
use fork::{Fork, ForkSchedule};
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
    consensus::{
        AggregatorCommitteeConsensusData, AssignedAggregator, BeaconVote, DataVersion,
        GloasBeaconVote, QbftDataValidator,
    },
};
use ssz::Encode;
use ssz_types::VariableList;
use task_executor::TaskExecutor;
use tempfile::TempDir;
use tokio::sync::watch;
use types::{
    Attestation, AttestationBase, AttestationData, ChainSpec, Checkpoint, Epoch, EthSpec, Graffiti,
    Hash256, MainnetEthSpec, SelectionProof, Slot,
};
use validator_store::{AggregateToSign, AttestationToSign, SyncMessageToSign};

use crate::{AggregationAssignments, AnchorValidatorStore, VotingAssignments, VotingContext};

pub(super) const TEST_SLOT: u64 = 1;
const SLOT_DURATION_SECS: u64 = 12;

// ==================== Mock consensus decider ====================

/// Mock that instantly returns `Completed::Success(initial)`, echoing back the proposed data.
/// Removes the need for `QbftManager` infrastructure and lets the signing pipeline run fully.
///
/// When `forced_gloas_index` is `Some`, a `GloasBeaconVote` seed is decided with its
/// `attestation_data_index` overridden to that value (all other fields preserved). This lets
/// tests exercise "the cluster-decided index differs from this operator's local seed", which is
/// exactly the case `#1027` must apply. Non-Gloas seeds (`BeaconVote`) are always echoed back
/// unchanged, since their decided value carries no index.
pub(super) struct MockConsensusDecider {
    forced_gloas_index: Option<u64>,
}

impl MockConsensusDecider {
    /// Echoes every decided seed back unchanged (default behavior used by most tests).
    pub(super) fn echoing() -> Self {
        Self {
            forced_gloas_index: None,
        }
    }

    /// Decides every `GloasBeaconVote` seed with its `attestation_data_index` replaced by
    /// `index`, modeling a cluster that agrees on an index that may differ from the local seed.
    pub(super) fn forcing_gloas_index(index: u64) -> Self {
        Self {
            forced_gloas_index: Some(index),
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
        Ok(Completed::Success(initial))
    }
}

// ==================== Mock signature collector ====================

/// Shared storage for captured `sign_and_collect` calls.
pub(super) type CapturedCalls = Arc<Mutex<Vec<CapturedSignatureCall>>>;

pub(super) struct CapturedSignatureCall {
    pub(super) requester: SignatureRequester,
    pub(super) metadata: SignatureMetadata,
    /// Only the root is captured, not the full `ValidatorSigningData`, so the capture never
    /// holds key share material.
    pub(super) signing_root: Hash256,
}

/// Mock that captures calls and returns a canned infinity signature, or a configured failure.
struct MockSignatureCollector {
    captured: CapturedCalls,
    failure: Option<CollectionError>,
}

impl SignatureCollecting for MockSignatureCollector {
    fn sign_and_collect(
        &self,
        metadata: SignatureMetadata,
        requester: SignatureRequester,
        signing_data: ValidatorSigningData,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<Signature>, CollectionError>> + Send + '_>> {
        // Capture before failing so tests can assert that the collection attempt happened even
        // when the configured outcome is an error.
        self.captured.lock().push(CapturedSignatureCall {
            requester,
            metadata,
            signing_root: signing_data.root,
        });
        if let Some(failure) = self.failure.clone() {
            return Box::pin(async move { Err(failure) });
        }
        let sig = Signature::infinity().expect("infinity signature");
        Box::pin(async move { Ok(Arc::new(sig)) })
    }
}

/// Creates a mock signature collector and returns the shared captured calls handle.
fn create_mock_collector(
    failure: Option<CollectionError>,
) -> (Box<dyn SignatureCollecting>, CapturedCalls) {
    let captured: CapturedCalls = Arc::new(Mutex::new(Vec::new()));
    let mock = MockSignatureCollector {
        captured: Arc::clone(&captured),
        failure,
    };
    (Box::new(mock), captured)
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

/// Construction knobs that only some tests need to vary.
pub(super) struct HarnessOptions {
    /// When set, every `sign_and_collect` call fails with this error after being captured.
    pub(super) collector_failure: Option<CollectionError>,
    pub(super) disable_slashing_protection: bool,
    /// Chain spec to wire into the store. Defaults to `ChainSpec::mainnet()`, under which
    /// `TEST_SLOT` is pre-Electra. Tests that need a specific fork at `TEST_SLOT` (Electra or
    /// Gloas) supply a spec with the matching `*_fork_epoch` set to genesis.
    pub(super) spec: Arc<ChainSpec>,
    /// When `Some`, the mock decides every `GloasBeaconVote` with this `attestation_data_index`,
    /// modeling a cluster-decided index that may differ from each operator's local seed.
    pub(super) forced_gloas_index: Option<u64>,
}

impl Default for HarnessOptions {
    fn default() -> Self {
        Self {
            collector_failure: None,
            // Slashing protection is disabled by default because the harness never registers
            // validators in the slashing DB, which would fail block/attestation signing paths.
            disable_slashing_protection: true,
            spec: Arc::new(ChainSpec::mainnet()),
            forced_gloas_index: None,
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
    pub(super) is_synced_tx: watch::Sender<bool>,
    /// The spec the store was built with, exposed so tests can recompute signing domains
    /// without duplicating the store's fork-selection logic.
    pub(super) spec: Arc<ChainSpec>,
    /// Genesis validators root the store was built with (`Hash256::zero()`), needed alongside
    /// `spec` to recompute signing roots.
    pub(super) genesis_validators_root: Hash256,
    _slashing_db_dir: TempDir,
    _exit_signal: async_channel::Sender<()>,
}

impl ValidatorStoreTestHarness {
    pub(super) fn new(committee_setups: Vec<CommitteeSetup>, our_operator_id: OperatorId) -> Self {
        Self::new_with_options(committee_setups, our_operator_id, HarnessOptions::default())
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
        slot_clock.set_current_time(Duration::from_secs(slot_start + SLOT_DURATION_SECS / 3 + 1));

        let (executor, exit_signal) = create_test_executor();

        let fork_schedule = Arc::new(ForkSchedule::new(
            Fork::Boole,
            ssv_types::domain_type::DomainType::default(),
            "test",
        ));

        let (mock_collector, captured_calls) = create_mock_collector(options.collector_failure);

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

        let decider = match options.forced_gloas_index {
            Some(index) => MockConsensusDecider::forcing_gloas_index(index),
            None => MockConsensusDecider::echoing(),
        };

        let spec = Arc::clone(&options.spec);
        let genesis_validators_root = Hash256::zero();

        let validator_store = AnchorValidatorStore::new(
            database,
            mock_collector,
            Arc::new(decider),
            slashing_protection,
            options.disable_slashing_protection,
            slot_clock.clone(),
            Arc::clone(&spec),
            genesis_validators_root,
            None, // impostor mode: no RSA decryption needed
            fork_schedule,
            30_000_000,
            None,
            false,
            false,
            is_synced_rx,
            executor,
        );

        Self {
            validator_store,
            committee_setups,
            captured_calls,
            is_synced_tx,
            spec,
            genesis_validators_root,
            _slashing_db_dir: slashing_db_dir,
            _exit_signal: exit_signal,
        }
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

    fn zero_checkpoint() -> Checkpoint {
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
        self.validator_store.update_voting_context(VotingContext {
            voting_assignments: Arc::new(self.test_slot_voting_assignments()),
            vote: crate::SlotVote::Gloas(vote),
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

    /// Seeds `AggregationAssignments` for the given committees at the provided slot.
    ///
    /// Each validator in the selected committee is treated as an attestation aggregator for a
    /// single beacon committee index derived from the test committee position.
    pub(super) fn seed_aggregation_assignments_for_slot(
        &self,
        slot: u64,
        committee_indices: &[usize],
    ) {
        let mut aggregator_committees = HashMap::new();
        let mut consensus_data_by_ssv_committee = HashMap::new();

        for &committee_idx in committee_indices {
            let setup = &self.committee_setups[committee_idx];
            let beacon_committee_index = committee_idx as u64;

            for validator in &setup.validators {
                aggregator_committees.insert(validator.public_key, beacon_committee_index);
            }

            let aggregators: Vec<_> = setup
                .validators
                .iter()
                .map(|validator| AssignedAggregator {
                    validator_index: validator.index.expect("test validator should have index"),
                    selection_proof: Signature::empty(),
                    committee_index: beacon_committee_index,
                })
                .collect();

            let aggregated_attestation = AttestationBase::<MainnetEthSpec> {
                aggregation_bits: ssz_types::BitList::with_capacity(128)
                    .expect("bitlist should be valid"),
                data: AttestationData {
                    slot: Slot::new(slot),
                    index: beacon_committee_index,
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
            };

            let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
                version: DataVersion::from(types::ForkName::Deneb),
                aggregators: VariableList::new(aggregators)
                    .expect("aggregator list should be valid"),
                aggregator_committee_indexes: VariableList::new(vec![beacon_committee_index])
                    .expect("committee indexes should be valid"),
                aggregated_attestations: VariableList::new(vec![
                    VariableList::new(aggregated_attestation.as_ssz_bytes())
                        .expect("attestation bytes should fit"),
                ])
                .expect("aggregated attestations should be valid"),
                contributors: VariableList::empty(),
                sync_committee_contributions: VariableList::empty(),
            };

            consensus_data_by_ssv_committee
                .insert(setup.cluster.committee_id(), Arc::new(consensus_data));
        }

        self.validator_store
            .update_aggregation_assignments(AggregationAssignments {
                slot: Slot::new(slot),
                aggregator_committees,
                multi_sync_aggregators: HashMap::new(),
                consensus_data_by_ssv_committee,
            });
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

    /// Builds an attestation duty for `TEST_SLOT` whose `data.index` is pre-set to `index`,
    /// modeling a BN-supplied attestation index (e.g. the committee index pre-Electra). Lets
    /// tests assert whether the signing path leaves that index untouched or overwrites it.
    pub(super) fn create_attestation_with_index(
        &self,
        committee_idx: usize,
        validator_idx: usize,
        index: u64,
    ) -> AttestationToSign<MainnetEthSpec> {
        let mut att = self.create_attestation(committee_idx, validator_idx);
        att.attestation.data_mut().index = index;
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
        self.create_aggregate_at_slot(committee_idx, validator_idx, TEST_SLOT)
    }

    pub(super) fn create_aggregate_at_slot(
        &self,
        committee_idx: usize,
        validator_idx: usize,
        slot: u64,
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
                    slot: Slot::new(slot),
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
