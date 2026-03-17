//! Shared test infrastructure for `AnchorValidatorStore` integration tests.
//!
//! Provides a `ValidatorStoreTestHarness` that wires up a real `AnchorValidatorStore` with
//! in-memory database, QBFT consensus, and signature collection. Supports multi-operator QBFT
//! routing so that consensus can complete across simulated operators.

use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU64,
    sync::Arc,
    time::Duration,
};

use bls::{AggregateSignature, FixedBytesExtended, PublicKeyBytes, SecretKey};
use bls_lagrange::{KeyId, split};
use database::NetworkDatabase;
use fork::{Fork, ForkSchedule};
use message_sender::testing::MockMessageSender;
use openssl::rsa::{Padding, Rsa};
use parking_lot::Mutex;
use processor::Senders;
use qbft_manager::{CommitteeInstanceId, QbftManager, TimeoutMode};
use signature_collector::SignatureCollectorManager;
use slashing_protection::SlashingDatabase;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    Cluster, ClusterId, CommitteeId, ENCRYPTED_KEY_LENGTH, IndexSet, OperatorId, Share,
    ValidatorIndex, ValidatorMetadata, VariableList,
    consensus::{BeaconVote, NoDataValidation, QbftMessage},
    domain_type::DomainType,
    message::{MsgType, SignedSSVMessage},
    msgid::DutyExecutor,
    partial_sig::{PartialSignatureMessage, PartialSignatureMessages, PartialSignatureMessagesLen},
};
use ssz::Decode;
use task_executor::TaskExecutor;
use tempfile::TempDir;
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use types::{
    Attestation, AttestationBase, AttestationData, ChainSpec, Checkpoint, Epoch, Graffiti, Hash256,
    MainnetEthSpec, Slot,
};
use validator_store::AttestationToSign;

use crate::{AnchorValidatorStore, VotingAssignments, VotingContext};

// ==================== Constants ====================

/// The slot used across all test attestations.
pub const TEST_SLOT: u64 = 1;

/// Slots per epoch (mainnet default).
pub const SLOTS_PER_EPOCH: u64 = 32;

/// RSA key size in bits for test key generation.
const RSA_KEY_SIZE: u32 = 2048;

/// Genesis duration in seconds for the manual slot clock.
const GENESIS_DURATION_SECS: u64 = 0;

/// Slot duration in seconds.
const SLOT_DURATION_SECS: u64 = 12;

// ==================== Key generation helpers ====================

/// A set of BLS key shares for a single validator, split across operators.
pub struct SplitKeySet {
    /// The validator's full BLS public key (derived from the master secret).
    pub validator_pubkey: PublicKeyBytes,
    /// Per-operator BLS secret key shares, keyed by `OperatorId`.
    pub shares: HashMap<OperatorId, SecretKey>,
}

/// Generates a Shamir-split BLS key for a validator across the given operators.
///
/// Uses `bls_lagrange::split` with threshold = 2f+1 (where f = (n-1)/3).
pub fn generate_split_keys(operator_ids: &[OperatorId]) -> SplitKeySet {
    let master = SecretKey::random();
    let validator_pubkey = PublicKeyBytes::from(master.public_key().compress());

    let num_operators = operator_ids.len() as u64;
    let f = (num_operators - 1) / 3;
    let threshold = 2 * f + 1;

    let key_ids: Vec<KeyId> = operator_ids
        .iter()
        .map(|op| KeyId::try_from(op.0).expect("operator ID should be non-zero"))
        .collect();

    let split_keys = split(&master, threshold, key_ids).expect("BLS key split should succeed");

    let shares: HashMap<OperatorId, SecretKey> = operator_ids
        .iter()
        .zip(split_keys.into_iter().map(|(_, sk)| sk))
        .map(|(op_id, sk)| (*op_id, sk))
        .collect();

    SplitKeySet {
        validator_pubkey,
        shares,
    }
}

/// RSA-encrypts a BLS secret key in the format expected by `decrypt_key_share`:
/// PKCS1-encrypted hex string of the 32-byte secret key.
pub fn rsa_encrypt_bls_key(
    secret_key: &SecretKey,
    rsa_pubkey: &Rsa<openssl::pkey::Public>,
) -> [u8; ENCRYPTED_KEY_LENGTH] {
    let key_bytes = secret_key.serialize();
    let hex_str = hex::encode(key_bytes);
    let hex_bytes = hex_str.as_bytes();

    let mut encrypted = vec![0u8; rsa_pubkey.size() as usize];
    let len = rsa_pubkey
        .public_encrypt(hex_bytes, &mut encrypted, Padding::PKCS1)
        .expect("RSA encryption of BLS key should succeed");

    assert_eq!(
        len, ENCRYPTED_KEY_LENGTH,
        "RSA encrypted output should be exactly {ENCRYPTED_KEY_LENGTH} bytes"
    );

    let mut result = [0u8; ENCRYPTED_KEY_LENGTH];
    result.copy_from_slice(&encrypted[..ENCRYPTED_KEY_LENGTH]);
    result
}

// ==================== Committee setup ====================

/// Describes one SSV committee for test setup: its cluster, validators, and key shares.
pub struct CommitteeSetup {
    pub cluster: Cluster,
    pub validators: Vec<ValidatorMetadata>,
    /// All shares for this cluster, across all operators and validators.
    pub shares: Vec<Share>,
    /// Raw BLS secret key shares: `(OperatorId, validator_pubkey)` -> `SecretKey`.
    /// Needed by the consensus router to simulate other operators' partial signatures.
    pub bls_shares: HashMap<(OperatorId, PublicKeyBytes), SecretKey>,
}

/// Creates a committee with the given operators and validator count.
///
/// Each validator gets a Shamir-split BLS key, with shares RSA-encrypted using
/// `our_rsa_pubkey` for the operator matching `our_operator_id`, and zeros for others
/// (since only our operator needs to decrypt).
pub fn create_committee_setup(
    operator_ids: &[OperatorId],
    num_validators: usize,
    our_operator_id: OperatorId,
    our_rsa_pubkey: &Rsa<openssl::pkey::Public>,
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
    let mut bls_shares = HashMap::new();

    for i in 0..num_validators {
        let split_keys = generate_split_keys(operator_ids);
        let validator_index = starting_validator_index + i;

        let validator = ValidatorMetadata {
            public_key: split_keys.validator_pubkey,
            cluster_id,
            index: Some(ValidatorIndex(validator_index)),
            graffiti: Graffiti::default(),
        };

        // Create shares for each operator and store raw BLS shares
        for &op_id in operator_ids {
            let sk = split_keys
                .shares
                .get(&op_id)
                .expect("share should exist for every operator");

            bls_shares.insert((op_id, split_keys.validator_pubkey), sk.clone());

            // Only encrypt with RSA for our operator; others get zeros (unused in tests)
            let encrypted_private_key = if op_id == our_operator_id {
                rsa_encrypt_bls_key(sk, our_rsa_pubkey)
            } else {
                [0u8; ENCRYPTED_KEY_LENGTH]
            };

            shares.push(Share {
                validator_pubkey: split_keys.validator_pubkey,
                operator_id: op_id,
                cluster_id,
                share_pubkey: PublicKeyBytes::from(sk.public_key().compress()),
                encrypted_private_key,
            });
        }

        validators.push(validator);
    }

    CommitteeSetup {
        cluster,
        validators,
        shares,
        bls_shares,
    }
}

// ==================== Test harness ====================

/// A test harness that creates a real `AnchorValidatorStore` wired to QBFT and signature
/// collection infrastructure.
///
/// Supports multi-operator QBFT consensus via `start_consensus_router()`, which creates
/// `QbftManager` instances for all operators and routes messages between them.
pub struct ValidatorStoreTestHarness {
    pub validator_store: Arc<AnchorValidatorStore<ManualSlotClock, MainnetEthSpec>>,
    /// Committee setups used to populate the database, indexed for reference.
    pub committee_setups: Vec<CommitteeSetup>,
    /// All operators' `QbftManager` instances (for multi-operator QBFT routing).
    pub qbft_managers: HashMap<OperatorId, Arc<QbftManager<MainnetEthSpec, ManualSlotClock>>>,
    /// Our operator's signature collector (for feeding partial sigs from other operators).
    pub signature_collector: Arc<SignatureCollectorManager<ManualSlotClock>>,
    /// The processor senders (for spawning background tasks).
    pub senders: Senders,
    /// Receives outgoing messages from QBFT and signature collector.
    /// Stored as `Option` so `start_consensus_router` can take ownership.
    pub network_rx: Option<mpsc::UnboundedReceiver<SignedSSVMessage>>,
    /// Controls the `is_synced` state seen by the validator store.
    pub is_synced_tx: watch::Sender<bool>,
    /// Slot clock shared across all components.
    pub slot_clock: ManualSlotClock,
    /// Temp directory for slashing DB (must be kept alive for the DB lifetime).
    #[expect(dead_code)]
    slashing_db_dir: TempDir,
    /// Our operator ID.
    pub our_operator_id: OperatorId,
}

impl ValidatorStoreTestHarness {
    /// Creates a new test harness with the specified committee setups.
    ///
    /// `our_operator_id` is the operator this node acts as. An RSA keypair is generated
    /// and used for share encryption/decryption.
    ///
    /// The clock is positioned just past the 1/3 mark of `TEST_SLOT`, which is the correct
    /// position for attestation QBFT timing. Tests that need instant QBFT timeouts (e.g. single-
    /// operator tests) should advance the clock further via `slot_clock.set_current_time`.
    pub fn new(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        rsa_private_key: Rsa<openssl::pkey::Private>,
        executor: TaskExecutor,
    ) -> Self {
        // Slot clock: genesis at time 0, slot duration 12s
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            Duration::from_secs(GENESIS_DURATION_SECS),
            Duration::from_secs(SLOT_DURATION_SECS),
        );
        // Position the clock just past the 1/3 mark of the test slot. This gives
        // `get_instant_in_slot(slot, slot_duration/3)` a recent `instance_start_time`,
        // allowing QBFT round 1 to complete before its 2s timeout.
        let slot_start = TEST_SLOT * SLOT_DURATION_SECS;
        let past_one_third = slot_start + SLOT_DURATION_SECS / 3 + 1;
        slot_clock.set_current_time(Duration::from_secs(past_one_third));

        // Mock network channel shared by all operators
        let (network_tx, network_rx) = mpsc::unbounded_channel();

        // Processor for QBFT and signature collector
        let processor_config = processor::Config {
            max_workers: 15,
            queue_size: Default::default(),
        };
        let senders: Senders = processor::spawn(processor_config, executor.clone());

        // Fork schedule
        let fork_schedule = Arc::new(ForkSchedule::new(
            Fork::Boole,
            DomainType::default(),
            "test",
        ));

        // Collect unique operator IDs across all committees
        let mut all_operator_ids = HashSet::new();
        for setup in &committee_setups {
            for &op_id in &setup.cluster.cluster_members {
                all_operator_ids.insert(op_id);
            }
        }

        // Create a QbftManager for every operator, all sharing the same `network_tx`
        let mut qbft_managers = HashMap::new();
        for &op_id in &all_operator_ids {
            let msg_sender = Arc::new(MockMessageSender::new(network_tx.clone(), op_id));
            let manager = QbftManager::new(
                senders.clone(),
                op_id.into(),
                slot_clock.clone(),
                msg_sender,
                NonZeroU64::new(SLOTS_PER_EPOCH).expect("slots per epoch is non-zero"),
                fork_schedule.clone(),
            )
            .expect("QbftManager creation should succeed");
            qbft_managers.insert(op_id, manager);
        }

        // Our operator's message sender (shared by QbftManager and SignatureCollector)
        let our_message_sender = Arc::new(MockMessageSender::new(network_tx, our_operator_id));

        // Signature collector for our operator
        let signature_collector = SignatureCollectorManager::new(
            senders.clone(),
            our_operator_id.into(),
            fork_schedule.clone(),
            SLOTS_PER_EPOCH,
            our_message_sender,
            slot_clock.clone(),
        )
        .expect("SignatureCollectorManager creation should succeed");

        // Database: create in-memory with our operator's RSA public key
        let our_rsa_pubkey = rsa_public_from_private(&rsa_private_key);

        let database = Arc::new(
            NetworkDatabase::new_in_memory(&our_rsa_pubkey, "test")
                .expect("in-memory database creation should succeed"),
        );

        // Populate the database with operators and validators from all committee setups
        {
            let mut conn = database
                .connection()
                .expect("database connection should succeed");
            let tx = conn.transaction().expect("transaction should start");

            let mut inserted_operators = std::collections::HashSet::new();
            for setup in &committee_setups {
                for &op_id in &setup.cluster.cluster_members {
                    if inserted_operators.insert(op_id) {
                        let operator = if op_id == our_operator_id {
                            ssv_types::Operator::new_with_pubkey(
                                our_rsa_pubkey.clone(),
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

                for (i, validator) in setup.validators.iter().enumerate() {
                    let validator_shares: Vec<Share> = setup
                        .shares
                        .iter()
                        .filter(|s| s.validator_pubkey == validator.public_key)
                        .cloned()
                        .collect();

                    database
                        .insert_validator(setup.cluster.clone(), validator, validator_shares, &tx)
                        .unwrap_or_else(|e| {
                            panic!("validator insertion should succeed for validator {i}: {e:?}")
                        });
                }
            }

            tx.commit().expect("transaction commit should succeed");
        }

        // Slashing protection DB (file-based, via tempdir)
        let slashing_db_dir = TempDir::new().expect("temp directory creation should succeed");
        let slashing_db_path = slashing_db_dir.path().join("slashing_protection.sqlite");
        let slashing_protection = Arc::new(
            SlashingDatabase::open_or_create(&slashing_db_path)
                .expect("slashing DB creation should succeed"),
        );

        // `is_synced` watch channel: default to synced
        let (is_synced_tx, is_synced_rx) = watch::channel(true);

        let spec = Arc::new(ChainSpec::mainnet());

        // Our operator's QbftManager goes to AnchorValidatorStore
        let our_qbft_manager = qbft_managers
            .get(&our_operator_id)
            .expect("our operator should have a QbftManager")
            .clone();

        let validator_store = AnchorValidatorStore::new(
            database,
            signature_collector.clone(),
            our_qbft_manager,
            slashing_protection,
            true, // disable_slashing_protection for simpler testing
            slot_clock.clone(),
            spec,
            Hash256::zero(),
            Some(rsa_private_key),
            fork_schedule,
            30_000_000, // gas_limit
            None,       // builder_boost_factor
            false,      // prefer_builder_proposals
            false,      // strict_mfp
            is_synced_rx,
            executor,
        );

        Self {
            validator_store,
            committee_setups,
            qbft_managers,
            signature_collector,
            senders,
            network_rx: Some(network_rx),
            is_synced_tx,
            slot_clock,
            slashing_db_dir,
            our_operator_id,
        }
    }

    /// Starts the multi-operator consensus router.
    ///
    /// This method:
    /// 1. Pre-starts QBFT instances on other operators for each enabled committee
    /// 2. Spawns a background task that routes consensus messages between all operators and
    ///    simulates partial signatures from other operators
    ///
    /// `enabled_committees` controls which committees have full multi-operator consensus.
    /// Committees not in this set will only have our operator's instance (causing QBFT timeout).
    ///
    /// Returns an `Arc<Mutex<Vec<SignedSSVMessage>>>` capturing all partial signature messages
    /// sent by our operator (useful for verifying batching behavior).
    pub fn start_consensus_router(
        &mut self,
        beacon_vote: BeaconVote,
        enabled_committees: &HashSet<CommitteeId>,
    ) -> Arc<Mutex<Vec<SignedSSVMessage>>> {
        let network_rx = self
            .network_rx
            .take()
            .expect("start_consensus_router can only be called once");

        // Pre-start QBFT instances on other operators for enabled committees
        for setup in &self.committee_setups {
            let committee_id = setup.cluster.committee_id();
            if !enabled_committees.contains(&committee_id) {
                continue;
            }

            let instance_id = CommitteeInstanceId {
                committee: committee_id,
                instance_height: (TEST_SLOT as usize).into(),
            };

            for &op_id in &setup.cluster.cluster_members {
                if op_id == self.our_operator_id {
                    // Our operator's instance is started by `sign_committee_attestations`
                    continue;
                }
                let manager = self.qbft_managers[&op_id].clone();
                let id = instance_id.clone();
                let vote = beacon_vote.clone();
                let members = setup.cluster.cluster_members.clone();
                self.senders
                    .permitless
                    .send_async(
                        async move {
                            let _ = manager
                                .decide_instance(
                                    id,
                                    vote,
                                    Box::new(NoDataValidation),
                                    TimeoutMode::SlotTime {
                                        instance_start_time: Instant::now(),
                                    },
                                    &members,
                                )
                                .await;
                        },
                        "test_qbft_other_operator",
                    )
                    .expect("spawning other operator QBFT instance should succeed");
            }
        }

        // Build router context
        let mut all_bls_shares = HashMap::new();
        let mut committee_members = HashMap::new();
        let mut validator_index_to_pubkey = HashMap::new();

        for setup in &self.committee_setups {
            let cid = setup.cluster.committee_id();
            committee_members.insert(cid, setup.cluster.cluster_members.clone());

            for ((op_id, pubkey), sk) in &setup.bls_shares {
                all_bls_shares.insert((*op_id, *pubkey), sk.clone());
            }

            for validator in &setup.validators {
                if let Some(idx) = validator.index {
                    validator_index_to_pubkey.insert(idx, validator.public_key);
                }
            }
        }

        let captured_partial_sigs = Arc::new(Mutex::new(Vec::new()));

        let ctx = RouterContext {
            managers: self.qbft_managers.clone(),
            sig_collector: self.signature_collector.clone(),
            bls_shares: all_bls_shares,
            committee_members,
            validator_index_to_pubkey,
            our_operator_id: self.our_operator_id,
            captured_partial_sigs: captured_partial_sigs.clone(),
        };

        tokio::spawn(async move {
            message_router(network_rx, ctx).await;
        });

        captured_partial_sigs
    }

    /// Seeds the `VotingContext` so that `get_voting_context` returns immediately for `TEST_SLOT`.
    ///
    /// Registers all validators from all committees as attesting validators.
    pub fn seed_voting_context(&self) {
        let mut attesting_validators = Vec::new();
        let mut attesting_committees = HashMap::new();

        for setup in &self.committee_setups {
            for (i, validator) in setup.validators.iter().enumerate() {
                if let Some(idx) = validator.index {
                    attesting_validators.push(idx);
                    attesting_committees.insert(validator.public_key, i as u64);
                }
            }
        }

        let voting_assignments = Arc::new(VotingAssignments {
            slot: Slot::new(TEST_SLOT),
            attesting_validators,
            attesting_committees,
            sync_validators_by_subnet: HashMap::new(),
        });

        let beacon_vote = BeaconVote {
            block_root: Hash256::zero(),
            source: Checkpoint {
                epoch: Epoch::new(0),
                root: Hash256::zero(),
            },
            target: Checkpoint {
                epoch: Epoch::new(0),
                root: Hash256::zero(),
            },
        };

        self.validator_store.update_voting_context(VotingContext {
            voting_assignments,
            beacon_vote,
        });
    }

    /// Constructs an `AttestationToSign` for a given validator in a committee at `TEST_SLOT`.
    pub fn create_attestation_to_sign(
        &self,
        committee_index: usize,
        validator_index_in_committee: usize,
    ) -> AttestationToSign<MainnetEthSpec> {
        self.create_attestation_to_sign_at_slot(
            committee_index,
            validator_index_in_committee,
            TEST_SLOT,
        )
    }

    /// Constructs an `AttestationToSign` at a specific slot.
    pub fn create_attestation_to_sign_at_slot(
        &self,
        committee_index: usize,
        validator_index_in_committee: usize,
        slot: u64,
    ) -> AttestationToSign<MainnetEthSpec> {
        let setup = &self.committee_setups[committee_index];
        let validator = &setup.validators[validator_index_in_committee];
        let validator_index = validator
            .index
            .expect("test validator should have an index");

        AttestationToSign {
            validator_index: *validator_index as u64,
            pubkey: validator.public_key,
            validator_committee_index: 0,
            attestation: Attestation::Base(AttestationBase {
                aggregation_bits: ssz_types::BitList::with_capacity(128)
                    .expect("bitlist capacity should be valid"),
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

    /// Advances the clock far past all QBFT timeouts so single-operator consensus
    /// times out instantly. Use this in tests that do NOT start the consensus router.
    pub fn set_clock_for_instant_timeout(&self) {
        let slot_start = TEST_SLOT * SLOT_DURATION_SECS;
        let one_third_mark = slot_start + SLOT_DURATION_SECS / 3;
        // Must exceed the QBFT max cumulative timeout for 12 rounds:
        // quick rounds 1-8 (8*2=16s) + slow rounds 9-12 (4*120=480s) = 496s
        let far_future = one_third_mark + 500;
        self.slot_clock
            .set_current_time(Duration::from_secs(far_future));
    }
}

/// Generates a fresh RSA keypair for testing.
pub fn generate_rsa_keypair() -> Rsa<openssl::pkey::Private> {
    Rsa::generate(RSA_KEY_SIZE).expect("RSA key generation should succeed")
}

/// Extracts the public key from an RSA private key.
pub fn rsa_public_from_private(
    private: &Rsa<openssl::pkey::Private>,
) -> Rsa<openssl::pkey::Public> {
    let pem = private
        .public_key_to_pem()
        .expect("RSA public key PEM export should succeed");
    Rsa::public_key_from_pem(&pem).expect("RSA public key PEM import should succeed")
}

/// Creates a `TaskExecutor` for use in async tests.
///
/// Returns the executor and a signal sender. Dropping the signal sender triggers shutdown.
pub fn create_test_executor() -> (TaskExecutor, async_channel::Sender<()>) {
    let handle = tokio::runtime::Handle::current();
    let (signal, exit) = async_channel::bounded::<()>(1);
    let (shutdown, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown);
    (executor, signal)
}

// ==================== Consensus router ====================

/// Internal context for the background message router.
struct RouterContext {
    managers: HashMap<OperatorId, Arc<QbftManager<MainnetEthSpec, ManualSlotClock>>>,
    sig_collector: Arc<SignatureCollectorManager<ManualSlotClock>>,
    bls_shares: HashMap<(OperatorId, PublicKeyBytes), SecretKey>,
    committee_members: HashMap<CommitteeId, IndexSet<OperatorId>>,
    validator_index_to_pubkey: HashMap<ValidatorIndex, PublicKeyBytes>,
    our_operator_id: OperatorId,
    captured_partial_sigs: Arc<Mutex<Vec<SignedSSVMessage>>>,
}

/// Background task that routes messages between all operators' QBFT instances and
/// simulates other operators' partial signatures.
///
/// Handles two message types:
/// - `SSVConsensusMsgType`: QBFT consensus messages routed to all operators via `receive_data`
/// - `SSVPartialSignatureMsgType`: partial signatures from our operator; simulates other operators
///   signing the same roots and feeds them into the signature collector
async fn message_router(
    mut network_rx: mpsc::UnboundedReceiver<SignedSSVMessage>,
    ctx: RouterContext,
) {
    while let Some(signed) = network_rx.recv().await {
        let msg_type = *signed.ssv_message().msg_type();

        match msg_type {
            MsgType::SSVConsensusMsgType => {
                route_consensus_message(&signed, &ctx);
            }
            MsgType::SSVPartialSignatureMsgType => {
                route_partial_signature(&signed, &ctx);
            }
        }
    }
}

/// Routes a QBFT consensus message to all operators in the relevant committee.
fn route_consensus_message(signed: &SignedSSVMessage, ctx: &RouterContext) {
    // Skip aggregated commits (multi-signature messages broadcast after consensus)
    if signed.signatures().len() > 1 {
        return;
    }

    // Decode the SSV QbftMessage from the SSVMessage data
    let qbft_message = match QbftMessage::from_ssz_bytes(signed.ssv_message().data()) {
        Ok(msg) => msg,
        Err(_) => return,
    };

    // Determine which committee this message belongs to
    let msg_id = signed.ssv_message().msg_id();
    let committee_id = match msg_id.duty_executor() {
        Some(DutyExecutor::Committee(cid)) => cid,
        _ => return,
    };

    // Route to all operators in this committee via the public `receive_data` API
    if let Some(members) = ctx.committee_members.get(&committee_id) {
        for op_id in members {
            if let Some(manager) = ctx.managers.get(op_id) {
                let _ = manager.receive_data(signed.clone(), qbft_message.clone());
            }
        }
    }
}

/// Handles partial signature messages from our operator by simulating other operators
/// signing the same roots and feeding the results into the signature collector.
fn route_partial_signature(signed: &SignedSSVMessage, ctx: &RouterContext) {
    // Only process messages from our operator
    let sender = signed.operator_ids().first().copied();
    if sender != Some(ctx.our_operator_id) {
        return;
    }

    // Capture this message for test assertions
    ctx.captured_partial_sigs.lock().push(signed.clone());

    // Decode the partial signature messages
    let partial_sigs = match PartialSignatureMessages::from_ssz_bytes(signed.ssv_message().data()) {
        Ok(msgs) => msgs,
        Err(_) => return,
    };

    // Determine which committee this belongs to
    let msg_id = signed.ssv_message().msg_id();
    let committee_id = match msg_id.duty_executor() {
        Some(DutyExecutor::Committee(cid)) => cid,
        _ => return,
    };

    let members = match ctx.committee_members.get(&committee_id) {
        Some(m) => m,
        None => return,
    };

    // For each other operator in the committee, sign the same roots
    for op_id in members {
        if *op_id == ctx.our_operator_id {
            continue;
        }

        let mut other_sigs = Vec::new();
        for msg in &partial_sigs.messages {
            // Map `validator_index` -> `pubkey` to look up this operator's BLS share
            let pubkey = match ctx.validator_index_to_pubkey.get(&msg.validator_index) {
                Some(pk) => pk,
                None => continue,
            };

            let secret_key = match ctx.bls_shares.get(&(*op_id, *pubkey)) {
                Some(sk) => sk,
                None => continue,
            };

            // Sign the same signing root with this operator's BLS share
            let signature = secret_key.sign(msg.signing_root);

            other_sigs.push(PartialSignatureMessage {
                partial_signature: signature,
                signing_root: msg.signing_root,
                signer: *op_id,
                validator_index: msg.validator_index,
            });
        }

        if other_sigs.is_empty() {
            continue;
        }

        let messages =
            VariableList::<_, PartialSignatureMessagesLen>::new(other_sigs).expect("within bounds");

        let other_partial_sigs = PartialSignatureMessages {
            kind: partial_sigs.kind,
            slot: partial_sigs.slot,
            messages,
        };

        // Feed other operators' partial signatures into our signature collector
        let _ = ctx
            .sig_collector
            .receive_partial_signatures(other_partial_sigs);
    }
}
