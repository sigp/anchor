use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    str::FromStr,
    sync::Arc,
};

use alloy::primitives::{Address, Bytes};
use database::test_utils::queries;
use eth::{
    SlashingProtection,
    event_processor::{EventProcessor, Mode},
    util::compute_cluster_id,
};
use ssv_types::*;

mod common;

use common::*;

#[derive(Debug, Default)]
struct PanicSlashingProtection;

impl SlashingProtection for PanicSlashingProtection {
    fn register_validator(&self, _public_key: bls::PublicKeyBytes) -> Result<(), String> {
        panic!("intentional panic for block-boundary regression test");
    }
}

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

/// Ensures a single `process_logs` call flushes block `N` before processing block `N+1`.
///
/// This is the core new behavior in PR2: a validator added in the second block of the fetched
/// batch must be able to observe the operators committed from the first block in that same call.
#[tokio::test]
async fn test_cross_block_operator_and_validator_processing() {
    setup_tracing();

    // Arrange: build one fetched batch containing operator events in block N and a validator add
    // in block N+1 that depends on those operators.
    let mut test = ProcessorFixture::new_empty();
    let cluster_owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");
    let operator_ids = vec![1u64, 2u64, 3u64, 4u64];
    let operator_block = 12363;
    let validator_block = 12364;
    let mut logs = Vec::new();

    for (log_index, operator_id) in operator_ids.iter().enumerate() {
        logs.push(create_operator_added_log_at_position(
            *operator_id,
            Address::random(),
            create_valid_rsa_public_key_bytes(),
            1000 + *operator_id,
            operator_block,
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
        validator_block,
        0,
        0,
    ));

    // Act: process both blocks together in one fetched batch.
    let result = test.processor.process_logs(logs, true, validator_block);

    // Assert: the validator add succeeds, proving block N was committed before block N+1 ran.
    assert!(
        result.is_ok(),
        "cross-block operator and validator processing should succeed"
    );

    for operator_id in operator_ids {
        verify_operator_stored(&test.processor, OperatorId(operator_id));
    }

    let validator_pubkey_str = format!("0x{}", hex::encode(validator_pubkey_bytes.serialize()));
    verify_validator_added(&test.processor, &validator_pubkey_str);
    verify_cluster_created(&test.processor, cluster_owner, &[1u64, 2u64, 3u64, 4u64]);
    assert_eq!(
        test.processor.db.state().get_last_processed_block(),
        validator_block
    );

    tokio::select! {
        validator_key = test.index_sync_rx.recv() => {
            assert_eq!(
                validator_key,
                Some(validator_pubkey_bytes),
                "validator should be queued for index sync after the later block commits"
            );
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
            panic!("validator should have been queued for index sync");
        }
    }
}

/// Ensures a fatal error in block `N+1` does not roll back the already flushed state from block
/// `N` in the same `process_logs` call.
#[tokio::test]
async fn test_cross_block_failure_preserves_previous_block_commit() {
    setup_tracing();

    // Arrange: first block adds operators, second block panics during validator registration.
    // Catching that unwind lets us distinguish a real block-boundary commit from "one tx for the
    // whole fetched batch", because block `N` should remain committed afterwards.
    let fixture = InMemoryTestFixture::new_empty();
    let (index_sync_tx, _index_sync_rx) = tokio::sync::mpsc::unbounded_channel();
    let (exit_tx, _exit_rx) = tokio::sync::mpsc::unbounded_channel();
    let db = Arc::new(fixture.data.db);
    let processor = EventProcessor::new(
        Arc::clone(&db),
        Mode::Node {
            index_sync_tx,
            exit_tx,
            slashing_protection: Arc::new(PanicSlashingProtection),
        },
    );

    let operator_ids = vec![1u64, 2u64, 3u64, 4u64];
    let operator_block = 12365;
    let panicking_block = 12366;
    let cluster_owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");
    let (shares, validator_pubkey_bytes) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, cluster_owner, 0);
    let validator_public_key = Bytes::from(validator_pubkey_bytes.serialize().to_vec());
    let mut logs = Vec::new();

    for (log_index, operator_id) in operator_ids.iter().enumerate() {
        logs.push(create_operator_added_log_at_position(
            *operator_id,
            Address::random(),
            create_valid_rsa_public_key_bytes(),
            1000 + *operator_id,
            operator_block,
            0,
            log_index as u64,
        ));
    }

    logs.push(create_validator_added_log_at_position(
        cluster_owner,
        operator_ids.clone(),
        validator_public_key,
        shares,
        panicking_block,
        0,
        0,
    ));

    // Act: process both blocks in one fetched batch and catch the intentional panic from block
    // N+1.
    let result = catch_unwind(AssertUnwindSafe(|| {
        processor.process_logs(logs, true, panicking_block)
    }));

    // Assert: block N+1 panics before its transaction commits, but block N remains durably
    // committed.
    assert!(
        result.is_err(),
        "the second block should panic during slashing registration"
    );

    for operator_id in operator_ids {
        verify_operator_stored(&processor, OperatorId(operator_id));
    }

    assert_eq!(
        db.state().get_last_processed_block(),
        operator_block,
        "the committed progress boundary should stop at the last successfully flushed block"
    );
}

/// Ensures `process_logs` still advances progress through `end_block` when the fetched range ends
/// on blocks that contain no relevant logs.
#[tokio::test]
async fn test_cross_block_empty_tail_advances_processed_block() {
    setup_tracing();

    // Arrange: only the first block in the fetched range has relevant logs.
    let test = ProcessorFixture::new_empty();
    let operator_block = 12367;
    let end_block = 12369;
    let operator_id = 1u64;
    let log = create_operator_added_log_at_position(
        operator_id,
        Address::random(),
        create_valid_rsa_public_key_bytes(),
        1000 + operator_id,
        operator_block,
        0,
        0,
    );

    // Act: process a range whose final block is empty from Anchor's point of view.
    let result = test.processor.process_logs(vec![log], true, end_block);

    // Assert: the relevant log commits, and progress still advances through the empty tail.
    assert!(
        result.is_ok(),
        "processing should succeed even when the fetched range ends on empty blocks"
    );
    verify_operator_stored(&test.processor, OperatorId(operator_id));
    assert_eq!(
        test.processor.db.state().get_last_processed_block(),
        end_block,
        "the empty tail block should still be marked as processed"
    );
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

#[tokio::test]
async fn test_validator_exit_channel_failure_is_non_fatal() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();
    let owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");
    let operator_ids = vec![1u64, 2u64, 3u64, 4u64];

    let operator_logs: Vec<_> = operator_ids
        .iter()
        .map(|operator_id| {
            create_operator_added_log(
                *operator_id,
                Address::random(),
                create_valid_rsa_public_key_bytes(),
                1000 + *operator_id,
            )
        })
        .collect();
    let operator_result = test.processor.process_logs(operator_logs, true, 12400);
    assert!(
        operator_result.is_ok(),
        "operator setup batch should succeed"
    );

    let (shares, validator_pubkey_bytes) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 0);
    let public_key = Bytes::from(validator_pubkey_bytes.serialize().to_vec());
    let add_result = test.processor.process_logs(
        vec![create_validator_added_log(
            owner,
            operator_ids.clone(),
            public_key.clone(),
            shares,
        )],
        true,
        12401,
    );
    assert!(add_result.is_ok(), "validator setup event should succeed");
    test.processor
        .db
        .set_validator_indices(HashMap::from([(
            validator_pubkey_bytes,
            ValidatorIndex(123),
        )]))
        .expect("validator index should be set before exit processing");

    let result = test.processor.process_logs(
        vec![create_validator_exited_log(owner, operator_ids, public_key)],
        true,
        12402,
    );
    assert!(
        result.is_ok(),
        "post-commit exit send failures should be logged and not returned"
    );
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12402);
}
