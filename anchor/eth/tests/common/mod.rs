//! Shared test utilities for integration tests.
//!
//! This module contains common helper functions and constants used across multiple integration
//! tests. Since integration tests are compiled separately, not all functions are used by every
//! test file, which would normally trigger dead_code warnings. The module-level allow attribute
//! acknowledges this expected behavior.

#![allow(dead_code)]

use std::sync::Arc;

use alloy::{
    primitives::{Address, Bytes, FixedBytes, LogData, U256},
    rpc::types::Log,
    sol_types::SolEvent,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bls::{Hash256, SecretKey};
use database::{
    NetworkDatabase,
    test_utils::{assertions, generators},
};
use eth::{
    event_processor::{EventProcessor, Mode},
    generated::SSVContract,
};
use slashing_protection::SlashingDatabase;
use ssv_types::*;
use tempfile::TempDir;
use tokio::sync::mpsc::unbounded_channel;
use types::PublicKeyBytes;

/// Test cluster owner address used across integration tests
pub const TEST_CLUSTER_OWNER: &str = "0x000000633b68f5d8d3a86593ebb815b4663bcbe0";

/// Generate a valid RSA public key as bytes for testing
pub fn create_valid_rsa_public_key_bytes() -> Bytes {
    let rsa_key = generators::pubkey::random_rsa();
    let pem_data = rsa_key
        .public_key_to_pem()
        .expect("Failed to convert to PEM");
    let base64_pem = BASE64_STANDARD.encode(&pem_data);
    Bytes::from(base64_pem.as_bytes().to_vec())
}

/// Generate valid shares data with correct signature verification
/// Returns (shares_data, validator_public_key) - both are needed for the test
pub fn create_valid_shares_data_for_owner_and_nonce(
    operator_ids: &[u64],
    owner: Address,
    nonce: u16,
) -> (Bytes, PublicKeyBytes) {
    let operator_count = operator_ids.len();

    // Constants from eth/src/util.rs
    const SIGNATURE_LENGTH: usize = 96;
    const PUBLIC_KEY_LENGTH: usize = 48;
    const ENCRYPTED_KEY_LENGTH: usize = 256;

    // Calculate expected length: signature + (public_keys * count) + (encrypted_keys * count)
    let expected_length = SIGNATURE_LENGTH
        + (PUBLIC_KEY_LENGTH * operator_count)
        + (ENCRYPTED_KEY_LENGTH * operator_count);

    let mut shares_bytes = Vec::with_capacity(expected_length);

    // 1. Generate a validator keypair for this test
    // For testing purposes, just generate a random key and use it
    let validator_secret_key = SecretKey::random();
    let validator_public_key = validator_secret_key.public_key();
    let validator_pubkey_bytes = validator_public_key.serialize();
    let validator_pubkey = PublicKeyBytes::deserialize(&validator_pubkey_bytes)
        .expect("Failed to deserialize validator public key");

    // Create the message that needs to be signed: "{owner}:{nonce}"
    let message_string = format!("{owner}:{nonce}");
    let message_hash = alloy::primitives::keccak256(message_string.as_bytes());
    let message = Hash256::from(message_hash.0);

    let signature = validator_secret_key.sign(message);
    shares_bytes.extend_from_slice(&signature.serialize());

    // 2. Add public keys (48 bytes each)
    for &_operator_id in operator_ids {
        // Generate public key for each operator
        let operator_key = SecretKey::random();
        let pub_key = operator_key.public_key();
        let pub_key_bytes = pub_key.serialize();
        shares_bytes.extend_from_slice(&pub_key_bytes);
    }

    // 3. Add encrypted private keys (256 bytes each)
    for &operator_id in operator_ids {
        // Generate deterministic encrypted key for each operator
        let mut encrypted_key = [0u8; ENCRYPTED_KEY_LENGTH];
        // Fill with deterministic data based on operator ID
        for (i, byte) in encrypted_key.iter_mut().enumerate() {
            *byte = ((operator_id as usize + i) % 256) as u8;
        }
        shares_bytes.extend_from_slice(&encrypted_key);
    }

    assert_eq!(
        shares_bytes.len(),
        expected_length,
        "Shares data length mismatch"
    );
    (Bytes::from(shares_bytes), validator_pubkey)
}

/// Create a test slashing database for EventProcessor setup
pub fn create_test_slashing_db() -> Arc<SlashingDatabase> {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let slashing_db_path = temp_dir.path().join("slashing.db");
    Arc::new(SlashingDatabase::create(&slashing_db_path).expect("Failed to create slashing db"))
}

/// Setup tracing for tests
pub fn setup_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

/// Create a Node mode EventProcessor with associated channels for testing
pub fn create_node_mode_processor(
    db: Arc<NetworkDatabase>,
) -> (
    EventProcessor,
    tokio::sync::mpsc::UnboundedReceiver<PublicKeyBytes>,
) {
    let (index_sync_tx, index_sync_rx) = unbounded_channel();
    let (exit_tx, _exit_rx) = unbounded_channel();
    let slashing_protection = create_test_slashing_db();

    let processor = EventProcessor::new(
        db,
        Mode::Node {
            index_sync_tx,
            exit_tx,
            slashing_protection,
        },
    );
    (processor, index_sync_rx)
}

/// Create a KeySplit mode EventProcessor for testing
pub fn create_keysplit_mode_processor(db: Arc<NetworkDatabase>) -> EventProcessor {
    EventProcessor::new(db, Mode::KeySplit)
}

/// Verify that an operator is properly stored in database and accessible from memory
pub fn verify_operator_stored(processor: &EventProcessor, operator_id: OperatorId) {
    // Get the stored operator from memory first (verifies memory accessibility)
    let stored_operator = processor
        .db
        .state()
        .get_operator(&operator_id)
        .expect("Operator should be stored and accessible");

    // Verify operator exists in database using database test utilities
    let mut conn = processor
        .db
        .connection()
        .expect("Failed to get database connection");
    let tx = conn.transaction().expect("Failed to start transaction");

    assertions::operator::exists_in_db(&stored_operator, &tx);
}

/// Helper function to create a mock Log object for SSV contract events
pub fn create_mock_log(
    address: Address,
    topics: Vec<FixedBytes<32>>,
    data: Bytes,
    block_number: Option<u64>,
    transaction_hash: Option<FixedBytes<32>>,
    log_index: Option<u64>,
) -> Log {
    let log_data = LogData::new(topics, data).expect("Failed to create log data");

    Log {
        inner: alloy::primitives::Log {
            address,
            data: log_data,
        },
        block_hash: Some(FixedBytes::default()),
        block_number,
        block_timestamp: Some(1234567890u64),
        transaction_hash,
        transaction_index: Some(0u64),
        log_index,
        removed: false,
    }
}

/// Helper function to encode operator ID as indexed topic
fn encode_operator_id_topic(operator_id: u64) -> FixedBytes<32> {
    let operator_id_bytes: [u8; 32] = {
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&operator_id.to_be_bytes());
        bytes
    };
    FixedBytes::from(operator_id_bytes)
}

/// Helper function to encode owner address as indexed topic
fn encode_owner_topic(owner: Address) -> FixedBytes<32> {
    let mut owner_bytes = [0u8; 32];
    owner_bytes[12..32].copy_from_slice(owner.as_slice());
    FixedBytes::from(owner_bytes)
}

/// Helper function to create an OperatorAdded event log
pub fn create_operator_added_log(
    operator_id: u64,
    owner: Address,
    public_key: Bytes,
    fee: u64,
) -> Log {
    let event = SSVContract::OperatorAdded {
        operatorId: operator_id,
        owner,
        publicKey: public_key,
        fee: U256::from(fee),
    };

    // Create topics array with the event signature and indexed parameters
    let mut topics = vec![SSVContract::OperatorAdded::SIGNATURE_HASH];
    topics.push(encode_operator_id_topic(operator_id));
    topics.push(encode_owner_topic(owner));

    // Encode the non-indexed data
    let data = event.encode_data();

    create_mock_log(
        Address::default(), // contract address
        topics,
        data.into(),
        Some(12345),
        Some(FixedBytes::default()),
        Some(0),
    )
}

/// Helper function to create a ValidatorAdded event log
pub fn create_validator_added_log(
    owner: Address,
    operator_ids: Vec<u64>,
    public_key: Bytes,
    shares: Bytes,
) -> Log {
    let cluster = SSVContract::Cluster {
        validatorCount: 1,
        networkFeeIndex: 0,
        index: 0,
        active: true,
        balance: U256::from(0),
    };

    let event = SSVContract::ValidatorAdded {
        owner,
        operatorIds: operator_ids,
        publicKey: public_key,
        shares,
        cluster,
    };

    // Create topics array with the event signature and indexed parameters
    let mut topics = vec![SSVContract::ValidatorAdded::SIGNATURE_HASH];
    topics.push(encode_owner_topic(owner)); // indexed owner

    // Encode the non-indexed data
    let data = event.encode_data();

    create_mock_log(
        Address::default(), // contract address
        topics,
        data.into(),
        Some(12346),
        Some(FixedBytes::default()),
        Some(1),
    )
}

/// Helper function to create an OperatorRemoved event log
pub fn create_operator_removed_log(operator_id: u64) -> Log {
    let _event = SSVContract::OperatorRemoved {
        operatorId: operator_id,
    };

    // Create topics array with the event signature and indexed parameters
    let mut topics = vec![SSVContract::OperatorRemoved::SIGNATURE_HASH];
    topics.push(encode_operator_id_topic(operator_id));

    // OperatorRemoved has no non-indexed data
    let data = Bytes::new();

    create_mock_log(
        Address::default(), // contract address
        topics,
        data,
        Some(12400),
        Some(FixedBytes::default()),
        Some(0),
    )
}

/// Helper function to create a ValidatorRemoved event log
pub fn create_validator_removed_log(
    owner: Address,
    operator_ids: Vec<u64>,
    public_key: Bytes,
) -> Log {
    let cluster = SSVContract::Cluster {
        validatorCount: 0, // 0 after removal
        networkFeeIndex: 0,
        index: 0,
        active: false, // inactive after removal
        balance: U256::from(0),
    };

    let event = SSVContract::ValidatorRemoved {
        owner,
        operatorIds: operator_ids,
        publicKey: public_key,
        cluster,
    };

    // Create topics array with the event signature and indexed parameters
    let mut topics = vec![SSVContract::ValidatorRemoved::SIGNATURE_HASH];
    topics.push(encode_owner_topic(owner)); // indexed owner

    // Encode the non-indexed data
    let data = event.encode_data();

    create_mock_log(
        Address::default(), // contract address
        topics,
        data.into(),
        Some(12401),
        Some(FixedBytes::default()),
        Some(2),
    )
}

/// Verify that an operator is soft deleted (removed from memory but still exists in database with
/// removed=TRUE)
pub fn verify_operator_soft_deleted(processor: &EventProcessor, operator_id: OperatorId) {
    use database::test_utils::assertions;

    // Verify operator is removed from memory state (soft delete removes from memory)
    assertions::operator::exists_not_in_memory(&processor.db, operator_id);

    // Verify operator is not accessible through normal database queries
    // (which filter out removed=TRUE operators)
    let mut conn = processor
        .db
        .connection()
        .expect("Failed to get database connection");
    let tx = conn.transaction().expect("Failed to start transaction");

    assertions::operator::exists_not_in_db(operator_id, &tx);

    // Verify operator still exists in database but is marked as removed=TRUE (soft delete)
    let is_soft_deleted = processor
        .db
        .is_operator_soft_deleted(operator_id, &tx)
        .expect("Failed to check if operator is soft deleted");

    assert!(
        is_soft_deleted,
        "Operator should be soft deleted (removed=TRUE) but still exist in database"
    );
}

/// Verify that an operator is hard deleted (completely removed from database)
pub fn verify_operator_hard_deleted(processor: &EventProcessor, operator_id: OperatorId) {
    use database::test_utils::assertions;

    // Verify operator is not in memory state
    assertions::operator::exists_not_in_memory(&processor.db, operator_id);

    // Verify operator does not exist in database at all (hard delete removes record completely)
    let mut conn = processor
        .db
        .connection()
        .expect("Failed to get database connection");
    let tx = conn.transaction().expect("Failed to start transaction");

    let operator_exists = processor
        .db
        .does_operator_exist(operator_id, &tx)
        .expect("Failed to check if operator exists in database");

    assert!(
        !operator_exists,
        "Operator should be hard deleted (completely removed from database)"
    );
}

/// Verify that a validator was successfully added and exists in the database
pub fn verify_validator_added(processor: &EventProcessor, validator_pubkey: &str) {
    use database::test_utils::queries;

    // Use the existing database query function to check if validator exists
    let mut conn = processor
        .db
        .connection()
        .expect("Failed to get database connection");
    let tx = conn.transaction().expect("Failed to start transaction");

    let validator_metadata = queries::get_validator(validator_pubkey, &tx);
    assert!(
        validator_metadata.is_some(),
        "Validator should exist in database"
    );
}

/// Verify that a cluster was created with the expected operators
pub fn verify_cluster_created(
    processor: &EventProcessor,
    cluster_owner: Address,
    operator_ids: &[u64],
) {
    use database::test_utils::queries;
    use eth::util::compute_cluster_id;

    // Compute the expected cluster ID using the same function as the event processor
    let cluster_id = compute_cluster_id(cluster_owner, operator_ids);

    // Use the existing database query function to check if cluster exists
    let mut conn = processor
        .db
        .connection()
        .expect("Failed to get database connection");
    let tx = conn.transaction().expect("Failed to start transaction");

    let cluster = queries::get_cluster(cluster_id, &tx);
    assert!(cluster.is_some(), "Cluster should exist in database");

    // For the test purposes, verifying that the cluster exists in the database
    // is sufficient to confirm it was created successfully with the expected operator set.
    // The cluster ID is computed from the owner and operator IDs, so if it exists,
    // it was created with the correct operators.
}
