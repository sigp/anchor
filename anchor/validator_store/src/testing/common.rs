//! Shared test infrastructure for `AnchorValidatorStore` integration tests.
//!
//! Provides a `ValidatorStoreTestHarness` that wires up a real `AnchorValidatorStore` with
//! in-memory database, QBFT consensus, and signature collection. Reusable across test modules
//! for different `sign_*` method tests.

use std::{collections::HashMap, num::NonZeroU64, sync::Arc, time::Duration};

use bls::{AggregateSignature, FixedBytesExtended, PublicKeyBytes, SecretKey};
use bls_lagrange::{KeyId, split};
use database::NetworkDatabase;
use fork::{Fork, ForkSchedule};
use message_sender::testing::MockMessageSender;
use openssl::rsa::{Padding, Rsa};
use processor::Senders;
use qbft_manager::QbftManager;
use signature_collector::SignatureCollectorManager;
use slashing_protection::SlashingDatabase;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    Cluster, ClusterId, ENCRYPTED_KEY_LENGTH, OperatorId, Share, ValidatorIndex, ValidatorMetadata,
    domain_type::DomainType,
};
use task_executor::TaskExecutor;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};
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

    for i in 0..num_validators {
        let split_keys = generate_split_keys(operator_ids);
        let validator_index = starting_validator_index + i;

        let validator = ValidatorMetadata {
            public_key: split_keys.validator_pubkey,
            cluster_id,
            index: Some(ValidatorIndex(validator_index)),
            graffiti: Graffiti::default(),
        };

        // Create shares for each operator
        for &op_id in operator_ids {
            let sk = split_keys
                .shares
                .get(&op_id)
                .expect("share should exist for every operator");

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
    }
}

// ==================== Test harness ====================

/// A test harness that creates a real `AnchorValidatorStore` wired to QBFT and signature
/// collection infrastructure.
///
/// The harness populates the database, sets up message routing channels, and provides
/// helpers for seeding `VotingContext` and constructing `AttestationToSign` values.
pub struct ValidatorStoreTestHarness {
    pub validator_store: Arc<AnchorValidatorStore<ManualSlotClock, MainnetEthSpec>>,
    /// Committee setups used to populate the database, indexed for reference.
    pub committee_setups: Vec<CommitteeSetup>,
    /// Receives outgoing messages from QBFT and signature collector.
    #[expect(dead_code)]
    pub network_rx: mpsc::UnboundedReceiver<ssv_types::message::SignedSSVMessage>,
    /// Controls the `is_synced` state seen by the validator store.
    pub is_synced_tx: watch::Sender<bool>,
    /// Slot clock shared across all components.
    #[expect(dead_code)]
    pub slot_clock: ManualSlotClock,
    /// Temp directory for slashing DB (must be kept alive for the DB lifetime).
    #[expect(dead_code)]
    slashing_db_dir: TempDir,
}

impl ValidatorStoreTestHarness {
    /// Creates a new test harness with the specified committee setups.
    ///
    /// `our_operator_id` is the operator this node acts as. An RSA keypair is generated
    /// and used for share encryption/decryption.
    pub fn new(
        committee_setups: Vec<CommitteeSetup>,
        our_operator_id: OperatorId,
        rsa_private_key: Rsa<openssl::pkey::Private>,
        executor: TaskExecutor,
    ) -> Self {
        // Slot clock: genesis at time 0, slot duration 12s, positioned at TEST_SLOT
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            Duration::from_secs(GENESIS_DURATION_SECS),
            Duration::from_secs(SLOT_DURATION_SECS),
        );
        // Position the clock ~10 minutes past the attestation slot's 1/3 mark.
        // This ensures all QBFT cumulative round timeouts are in the past, causing
        // rapid round cycling and eventual `Completed::TimedOut` without waiting
        // for real time to elapse.
        let slot_start = TEST_SLOT * SLOT_DURATION_SECS;
        let one_third_mark = slot_start + SLOT_DURATION_SECS / 3;
        // Must exceed the QBFT max cumulative timeout for 12 rounds:
        // quick rounds 1-8 (8*2=16s) + slow rounds 9-12 (4*120=480s) = 496s.
        let far_future = one_third_mark + 500;
        slot_clock.set_current_time(Duration::from_secs(far_future));

        // Mock network channel for outgoing messages
        let (network_tx, network_rx) = mpsc::unbounded_channel();
        let message_sender = Arc::new(MockMessageSender::new(network_tx, our_operator_id));

        // Processor for QBFT and signature collector
        let processor_config = processor::Config {
            max_workers: 15,
            queue_size: Default::default(),
        };
        let senders: Senders = processor::spawn(processor_config, executor.clone());

        // Fork schedule
        let fork_schedule = Arc::new(ForkSchedule::new(Fork::Alan, DomainType::default(), "test"));

        // QBFT manager for our operator
        let qbft_manager = QbftManager::new(
            senders.clone(),
            our_operator_id.into(),
            slot_clock.clone(),
            message_sender.clone(),
            NonZeroU64::new(SLOTS_PER_EPOCH).expect("slots per epoch is non-zero"),
            fork_schedule.clone(),
        )
        .expect("QbftManager creation should succeed");

        // Signature collector for our operator
        let signature_collector = SignatureCollectorManager::new(
            senders,
            our_operator_id.into(),
            fork_schedule.clone(),
            SLOTS_PER_EPOCH,
            message_sender,
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

            // Collect and deduplicate operators across committees.
            // Our operator must be inserted with the matching RSA public key so the
            // database recognizes it as "us" and stores shares in the in-memory state.
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
                    // Gather shares for this validator
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

        let validator_store = AnchorValidatorStore::new(
            database,
            signature_collector,
            qbft_manager,
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
            network_rx,
            is_synced_tx,
            slot_clock,
            slashing_db_dir,
        }
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
                    // Map pubkey to committee index (using position as a stand-in)
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

        let beacon_vote = ssv_types::consensus::BeaconVote {
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

    /// Constructs an `AttestationToSign` for a given validator in a committee.
    pub fn create_attestation_to_sign(
        &self,
        committee_index: usize,
        validator_index_in_committee: usize,
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
                    slot: Slot::new(TEST_SLOT),
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
