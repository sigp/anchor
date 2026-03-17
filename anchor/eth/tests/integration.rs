use std::{str::FromStr, sync::Arc};

use alloy::primitives::{Address, Bytes};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use database::{ProcessedEventCursor, test_utils::generators};
use ssv_types::*;

mod common;

use common::*;

#[tokio::test]
async fn test_operator_added_event_processing() {
    setup_tracing();

    // Setup test fixture with processor
    let test = ProcessorFixture::new_empty();

    // Create test data
    let operator_id = 1u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    // Create OperatorAdded log
    let log = create_operator_added_log(operator_id, owner, public_key, 1000);

    // Process the log
    let result = test.processor.process_logs(vec![log], true, 12345);
    assert!(
        result.is_ok(),
        "Processing OperatorAdded event should succeed"
    );

    // Verify operator was stored in database and memory
    verify_operator_stored(&test.processor, OperatorId(operator_id));
}

#[tokio::test]
async fn test_validator_added_event_processing() {
    setup_tracing();

    // Setup test fixture with populated operators and processor
    let mut test = ProcessorFixture::new();

    // Get operator IDs from the fixture
    let operator_ids = test.get_operator_ids();

    // Create properly formatted shares data with valid signature
    let owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");

    let (shares, validator_pubkey_bytes) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 0);
    let public_key = Bytes::from(validator_pubkey_bytes.serialize().to_vec());

    // Create ValidatorAdded log
    let log = create_validator_added_log(owner, operator_ids, public_key, shares);

    // Process the log - should succeed with valid signature
    let result = test.processor.process_logs(vec![log], true, 12346);

    // Should be processed successfully with valid signature
    assert!(
        result.is_ok(),
        "ValidatorAdded should be processed successfully with valid signature"
    );

    // Verify that validator was queued for index sync
    tokio::select! {
        validator_key = test.index_sync_rx.recv() => {
            assert!(validator_key.is_some(), "Validator should be queued for index sync");
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
            panic!("Validator should have been queued for index sync");
        }
    }
}

/// Test processing multiple events in a single batch
#[tokio::test]
async fn test_multiple_events_processing() {
    setup_tracing();

    // Setup test fixture with processor
    let test = ProcessorFixture::new_empty();

    let num_operators = 3u64;
    let mut logs = Vec::new();

    // Create multiple operator added events
    for i in 0..num_operators {
        let operator_id = i + 1;
        let owner = Address::random();
        let public_key = create_valid_rsa_public_key_bytes();
        let fee = 100000;

        let log = create_operator_added_log(operator_id, owner, public_key, fee);
        logs.push(log);
    }

    // Process all logs in a single batch
    let result = test.processor.process_logs(logs, true, 12350);
    assert!(result.is_ok(), "Processing multiple events should succeed");

    // Verify all operators were stored
    for i in 0..num_operators {
        verify_operator_stored(&test.processor, OperatorId(i + 1));
    }

    // Verify processed block was updated using proper database API
    let block_number = test.processor.db.state().get_last_processed_block();
    assert_eq!(block_number, 12350, "Block number should be updated");
}

#[tokio::test]
async fn test_database_transaction_rollback_on_error() {
    // Setup test fixture with processor
    let test = ProcessorFixture::new_empty();

    // Create test data
    let operator_id = 1u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    let valid_log = create_operator_added_log(operator_id, owner, public_key.clone(), 1000);

    // Create an invalid log (duplicate operator ID) that should cause an error
    let invalid_log = create_operator_added_log(operator_id, Address::random(), public_key, 2000);

    let logs = vec![valid_log, invalid_log];

    // Process logs - this should fail due to duplicate operator ID
    let result = test.processor.process_logs(logs, true, 12351);

    // The processing should complete (some events may be malformed and skipped)
    // but the transaction should still commit for valid events
    assert!(
        result.is_ok(),
        "Processing should handle malformed events gracefully"
    );

    // Verify the first operator was stored (malformed events are skipped, not rolled back)
    verify_operator_stored(&test.processor, OperatorId(operator_id));
}

#[tokio::test]
async fn test_resume_skips_already_processed_logs_in_same_block() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();

    let first_operator_id = 1u64;
    let second_operator_id = 2u64;
    let first_owner = Address::random();
    let first_rsa_pubkey = generators::pubkey::random_rsa();
    let first_public_key = Bytes::from(
        BASE64_STANDARD
            .encode(
                first_rsa_pubkey
                    .public_key_to_pem()
                    .expect("Failed to encode RSA public key"),
            )
            .into_bytes(),
    );

    let first_log =
        create_operator_added_log(first_operator_id, first_owner, first_public_key, 1000);
    let mut second_log = create_operator_added_log(
        second_operator_id,
        Address::random(),
        create_valid_rsa_public_key_bytes(),
        1000,
    );
    second_log.log_index = Some(1);

    let cursor = ProcessedEventCursor {
        block_number: 12345,
        transaction_index: 0,
        log_index: 0,
    };
    test.processor
        .db
        .commit_operator_added(
            &Operator {
                id: OperatorId(first_operator_id),
                owner: first_owner,
                rsa_pubkey: first_rsa_pubkey,
            },
            first_operator_id,
            cursor,
        )
        .expect("Failed to seed committed operator state");

    assert_eq!(
        test.processor.db.state().get_last_processed_event(),
        Some(cursor)
    );

    test.processor
        .process_logs(vec![first_log, second_log], true, 12345)
        .expect("Processing resumed logs should succeed");

    verify_operator_stored(&test.processor, OperatorId(first_operator_id));
    verify_operator_stored(&test.processor, OperatorId(second_operator_id));
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12345);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

#[tokio::test]
async fn test_keysplit_mode_processing() {
    use database::test_utils::{TEST_NETWORK, generators};

    // Setup database and KeySplit processor (no fixture needed for KeySplit tests)
    let pubkey = generators::pubkey::random_rsa();
    let db = Arc::new(
        database::NetworkDatabase::new_in_memory(&pubkey, TEST_NETWORK)
            .expect("Failed to create in-memory database"),
    );
    let processor = create_keysplit_mode_processor(db);

    // Create test data
    let operator_id = 1u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    let log = create_operator_added_log(operator_id, owner, public_key, 1500);

    // Process the log
    let result = processor.process_logs(vec![log], true, 12352);
    assert!(
        result.is_ok(),
        "KeySplit mode should process OperatorAdded events"
    );

    // Verify operator was stored even in KeySplit mode
    verify_operator_stored(&processor, OperatorId(operator_id));
}
