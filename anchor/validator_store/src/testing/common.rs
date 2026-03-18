//! Shared test infrastructure for `AnchorValidatorStore` integration tests.
//!
//! Provides a `ValidatorStoreTestHarness` that wires up a real `AnchorValidatorStore` with
//! in-memory database, single-operator QBFT, and a mock signature collector.

use std::{
    collections::HashMap, future::Future, num::NonZeroU64, pin::Pin, sync::Arc, time::Duration,
};

use bls::{AggregateSignature, FixedBytesExtended, PublicKeyBytes, Signature};
use database::NetworkDatabase;
use fork::{Fork, ForkSchedule};
use message_sender::testing::MockMessageSender;
use openssl::rsa::Rsa;
use parking_lot::Mutex;
use qbft_manager::QbftManager;
use signature_collector::{
    CollectionError, SignatureCollecting, SignatureMetadata, SignatureRequester,
    ValidatorSigningData,
};
use slashing_protection::SlashingDatabase;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    Cluster, ClusterId, ENCRYPTED_KEY_LENGTH, OperatorId, Share, ValidatorIndex, ValidatorMetadata,
};
use task_executor::TaskExecutor;
use tempfile::TempDir;
use tokio::sync::watch;
use types::{
    Attestation, AttestationBase, AttestationData, ChainSpec, Checkpoint, Epoch, Graffiti, Hash256,
    MainnetEthSpec, Slot,
};
use validator_store::AttestationToSign;

use crate::{AnchorValidatorStore, VotingAssignments, VotingContext};

pub const TEST_SLOT: u64 = 1;
const SLOTS_PER_EPOCH: u64 = 32;
const RSA_KEY_SIZE: u32 = 2048;
const SLOT_DURATION_SECS: u64 = 12;

// ==================== Mock signature collector ====================

/// Shared storage for captured `sign_and_collect` calls.
pub type CapturedCalls = Arc<Mutex<Vec<CapturedSignatureCall>>>;

pub struct CapturedSignatureCall {
    pub requester: SignatureRequester,
}

/// Mock that captures calls and returns a canned infinity signature.
struct MockSignatureCollector {
    captured: CapturedCalls,
}

impl SignatureCollecting for MockSignatureCollector {
    fn sign_and_collect(
        &self,
        _metadata: SignatureMetadata,
        requester: SignatureRequester,
        _signing_data: ValidatorSigningData,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<Signature>, CollectionError>> + Send + '_>> {
        self.captured
            .lock()
            .push(CapturedSignatureCall { requester });
        let sig = Signature::infinity().expect("infinity signature");
        Box::pin(async move { Ok(Arc::new(sig)) })
    }
}

/// Creates a mock signature collector and returns the shared captured calls handle.
pub fn create_mock_collector() -> (Box<dyn SignatureCollecting>, CapturedCalls) {
    let captured: CapturedCalls = Arc::new(Mutex::new(Vec::new()));
    let mock = MockSignatureCollector {
        captured: Arc::clone(&captured),
    };
    (Box::new(mock), captured)
}

// ==================== Committee setup ====================

pub struct CommitteeSetup {
    pub cluster: Cluster,
    pub validators: Vec<ValidatorMetadata>,
    pub shares: Vec<Share>,
}

pub fn create_committee_setup(
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

pub struct ValidatorStoreTestHarness {
    pub validator_store: Arc<AnchorValidatorStore<ManualSlotClock, MainnetEthSpec>>,
    pub committee_setups: Vec<CommitteeSetup>,
    pub captured_calls: CapturedCalls,
    pub is_synced_tx: watch::Sender<bool>,
    pub slot_clock: ManualSlotClock,
    _slashing_db_dir: TempDir,
    _exit_signal: async_channel::Sender<()>,
}

impl ValidatorStoreTestHarness {
    pub fn new(committee_setups: Vec<CommitteeSetup>, our_operator_id: OperatorId) -> Self {
        // Dummy RSA key for database operator identification (not used for decryption)
        let rsa_pubkey = {
            let rsa = Rsa::generate(RSA_KEY_SIZE).expect("RSA key generation should succeed");
            let pem = rsa
                .public_key_to_pem()
                .expect("RSA PEM export should succeed");
            Rsa::public_key_from_pem(&pem).expect("RSA PEM import should succeed")
        };

        // Slot clock positioned just past the 1/3 mark of TEST_SLOT
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            Duration::from_secs(0),
            Duration::from_secs(SLOT_DURATION_SECS),
        );
        let slot_start = TEST_SLOT * SLOT_DURATION_SECS;
        slot_clock.set_current_time(Duration::from_secs(slot_start + SLOT_DURATION_SECS / 3 + 1));

        // Minimal infrastructure for QbftManager (messages go nowhere)
        let (network_tx, _network_rx) = tokio::sync::mpsc::unbounded_channel();
        let processor_config = processor::Config {
            max_workers: 4,
            queue_size: Default::default(),
        };
        let (executor, exit_signal) = create_test_executor();
        let senders = processor::spawn(processor_config, executor.clone());

        let fork_schedule = Arc::new(ForkSchedule::new(
            Fork::Boole,
            ssv_types::domain_type::DomainType::default(),
            "test",
        ));

        let msg_sender = Arc::new(MockMessageSender::new(network_tx, our_operator_id));
        let qbft_manager = QbftManager::new(
            senders,
            our_operator_id.into(),
            slot_clock.clone(),
            msg_sender,
            NonZeroU64::new(SLOTS_PER_EPOCH).expect("non-zero"),
            fork_schedule.clone(),
        )
        .expect("QbftManager creation should succeed");

        let (mock_collector, captured_calls) = create_mock_collector();

        // Database
        let database = Arc::new(
            NetworkDatabase::new_in_memory(&rsa_pubkey, "test")
                .expect("in-memory database should succeed"),
        );

        {
            let mut conn = database.connection().expect("connection should succeed");
            let tx = conn.transaction().expect("transaction should start");

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
                            .insert_operator(&operator, &tx)
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
                        .insert_validator(setup.cluster.clone(), validator, validator_shares, &tx)
                        .expect("validator insertion should succeed");
                }
            }

            tx.commit().expect("commit should succeed");
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
            qbft_manager,
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
            false,
            is_synced_rx,
            executor,
        );

        Self {
            validator_store,
            committee_setups,
            captured_calls,
            is_synced_tx,
            slot_clock,
            _slashing_db_dir: slashing_db_dir,
            _exit_signal: exit_signal,
        }
    }

    /// Seeds the `VotingContext` so `get_voting_context` returns immediately for `TEST_SLOT`.
    pub fn seed_voting_context(&self) {
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

    pub fn create_attestation(
        &self,
        committee_idx: usize,
        validator_idx: usize,
    ) -> AttestationToSign<MainnetEthSpec> {
        self.create_attestation_at_slot(committee_idx, validator_idx, TEST_SLOT)
    }

    pub fn create_attestation_at_slot(
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

    /// Advances clock far past QBFT timeouts so single-operator consensus times out instantly.
    pub fn set_clock_for_instant_timeout(&self) {
        let one_third = TEST_SLOT * SLOT_DURATION_SECS + SLOT_DURATION_SECS / 3;
        // Exceeds QBFT max cumulative timeout (rounds 1-8: 16s + rounds 9-12: 480s = 496s)
        self.slot_clock
            .set_current_time(Duration::from_secs(one_third + 500));
    }
}

fn create_test_executor() -> (TaskExecutor, async_channel::Sender<()>) {
    let handle = tokio::runtime::Handle::current();
    let (signal, exit) = async_channel::bounded::<()>(1);
    let (shutdown, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown);
    (executor, signal)
}
