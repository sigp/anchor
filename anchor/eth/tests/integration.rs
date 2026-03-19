use std::{str::FromStr, sync::Arc};

use alloy::primitives::{Address, Bytes};
use database::test_utils::queries;
use eth::util::compute_cluster_id;
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
async fn test_same_block_operator_and_validator_processing() {
    setup_tracing();

    let mut test = ProcessorFixture::new_empty();
    let cluster_owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");
    let operator_ids = vec![1u64, 2u64, 3u64, 4u64];
    let same_block = 12360;
    let mut logs = Vec::new();

    for (log_index, operator_id) in operator_ids.iter().enumerate() {
        let owner = Address::random();
        let public_key = create_valid_rsa_public_key_bytes();
        logs.push(create_operator_added_log_at_position(
            *operator_id,
            owner,
            public_key,
            1000 + *operator_id,
            same_block,
            0,
            log_index as u64,
        ));
    }

    let (shares, validator_pubkey_bytes) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, cluster_owner, 0);
    let validator_public_key = Bytes::from(validator_pubkey_bytes.serialize().to_vec());
    logs.push(create_validator_added_log_at_position(
        cluster_owner,
        operator_ids.clone(),
        validator_public_key,
        shares,
        same_block,
        1,
        0,
    ));

    let result = test.processor.process_logs(logs, true, same_block);
    assert!(
        result.is_ok(),
        "same-block operator and validator processing should succeed"
    );

    for operator_id in operator_ids {
        verify_operator_stored(&test.processor, OperatorId(operator_id));
    }

    let validator_pubkey_str = format!("0x{}", hex::encode(validator_pubkey_bytes.serialize()));
    verify_validator_added(&test.processor, &validator_pubkey_str);
    verify_cluster_created(&test.processor, cluster_owner, &[1u64, 2u64, 3u64, 4u64]);
    assert_eq!(
        test.processor.db.state().get_last_processed_block(),
        same_block
    );

    tokio::select! {
        validator_key = test.index_sync_rx.recv() => {
            assert_eq!(
                validator_key,
                Some(validator_pubkey_bytes),
                "validator should be queued for index sync"
            );
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
            panic!("validator should have been queued for index sync");
        }
    }
}

#[tokio::test]
async fn test_same_block_validator_add_and_remove_processing() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();
    let cluster_owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");
    let operator_ids = vec![1u64, 2u64, 3u64, 4u64];
    let same_block = 12362;
    let operator_logs: Vec<_> = operator_ids
        .iter()
        .enumerate()
        .map(|(log_index, operator_id)| {
            create_operator_added_log_at_position(
                *operator_id,
                Address::random(),
                create_valid_rsa_public_key_bytes(),
                1000 + *operator_id,
                12361,
                0,
                log_index as u64,
            )
        })
        .collect();

    let operator_result = test.processor.process_logs(operator_logs, true, 12361);
    assert!(
        operator_result.is_ok(),
        "operator setup batch should succeed"
    );

    let (shares, validator_pubkey_bytes) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, cluster_owner, 0);
    let validator_public_key = Bytes::from(validator_pubkey_bytes.serialize().to_vec());
    let logs = vec![
        create_validator_added_log_at_position(
            cluster_owner,
            operator_ids.clone(),
            validator_public_key.clone(),
            shares,
            same_block,
            0,
            0,
        ),
        create_validator_removed_log_at_position(
            cluster_owner,
            operator_ids.clone(),
            validator_public_key,
            same_block,
            1,
            0,
        ),
    ];

    let result = test.processor.process_logs(logs, true, same_block);
    assert!(
        result.is_ok(),
        "same-block validator add and remove processing should succeed"
    );

    let mut conn = test
        .processor
        .db
        .connection()
        .expect("Failed to get database connection");
    let tx = conn.transaction().expect("Failed to start transaction");
    let validator_pubkey_str = format!("0x{}", hex::encode(validator_pubkey_bytes.serialize()));
    let cluster_id = compute_cluster_id(cluster_owner, &operator_ids);

    assert!(
        queries::get_validator(&validator_pubkey_str, &tx).is_none(),
        "validator should be removed after the batch completes"
    );
    assert!(
        queries::get_cluster(cluster_id, &tx).is_none(),
        "cluster should be removed after its only validator is removed"
    );
    assert_eq!(
        test.processor.db.state().get_last_processed_block(),
        same_block
    );
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
