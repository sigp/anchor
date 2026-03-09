use std::{collections::HashMap, str::FromStr, sync::Arc};

use alloy::primitives::{Address, Bytes};
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

#[tokio::test]
async fn test_validator_exit_processor_failure_aborts_batch() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();

    let operator_ids = vec![1u64, 2u64, 3u64, 4u64];
    let mut operator_logs = Vec::new();
    for operator_id in &operator_ids {
        operator_logs.push(create_operator_added_log(
            *operator_id,
            Address::random(),
            create_valid_rsa_public_key_bytes(),
            1_000,
        ));
    }

    let result = test.processor.process_logs(operator_logs, true, 12345);
    assert!(result.is_ok(), "Operator setup should succeed");

    let owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");
    let (shares, validator_pubkey) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 0);
    let validator_public_key = Bytes::from(validator_pubkey.serialize().to_vec());

    let validator_added_log = create_validator_added_log(
        owner,
        operator_ids.clone(),
        validator_public_key.clone(),
        shares,
    );
    let result = test
        .processor
        .process_logs(vec![validator_added_log], true, 12346);
    assert!(result.is_ok(), "Validator setup should succeed");

    test.processor
        .db
        .set_validator_indices(HashMap::from([(validator_pubkey, ValidatorIndex(42))]))
        .expect("Setting validator index should succeed");

    let exit_log =
        create_validator_exited_log(owner, operator_ids.clone(), validator_public_key.clone());
    let trailing_operator_log = create_operator_added_log(
        99,
        Address::random(),
        create_valid_rsa_public_key_bytes(),
        2_000,
    );

    let result = test
        .processor
        .process_logs(vec![exit_log, trailing_operator_log], true, 12347);
    let err = result.expect_err("Exit processor failure should abort the batch");

    assert!(
        err.to_string().contains("Exit processor unavailable"),
        "Unexpected error: {err}"
    );
    assert!(
        test.processor
            .db
            .state()
            .get_operator(&OperatorId(99))
            .is_none(),
        "Later logs in the aborted batch should not be applied"
    );
    assert_eq!(
        test.processor.db.state().get_last_processed_block(),
        12346,
        "Failed batches should not advance the processed block"
    );
}
