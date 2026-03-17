//! Focused `OperatorAdded` coverage for the per-event commit model.
//!
//! These tests cover:
//! - the happy path
//! - malformed/duplicate operator history that must still preserve `max_operator_id_seen`
//! - resume semantics within a partially processed block
//! - KeySplit behavior

use std::sync::Arc;

use alloy::primitives::Address;
use base64::Engine;
use database::test_utils::generators;
use ssv_types::{Operator, OperatorId};

mod common;

use common::*;

#[tokio::test]
/// A valid `OperatorAdded` log should persist the operator and advance progress normally.
async fn test_operator_added_event_processing() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();

    let operator_id = 1u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    let log = create_operator_added_log(operator_id, owner, public_key, 1000);

    let result = test.processor.process_logs(vec![log], true, 12345);
    assert!(
        result.is_ok(),
        "Processing OperatorAdded event should succeed"
    );

    verify_operator_stored(&test.processor, OperatorId(operator_id));
}

#[tokio::test]
/// Duplicate operator ids are malformed history, but they should not abort the batch or roll back
/// already-committed operators from the same fetched range.
async fn test_duplicate_operator_id_is_skipped() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();

    let operator_id = 1u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    let valid_log = create_operator_added_log(operator_id, owner, public_key.clone(), 1000);
    let duplicate_log = create_operator_added_log(operator_id, Address::random(), public_key, 2000);

    let result = test
        .processor
        .process_logs(vec![valid_log, duplicate_log], true, 12351);
    assert!(
        result.is_ok(),
        "Duplicate operator ids should be skipped without aborting the batch"
    );

    verify_operator_stored(&test.processor, OperatorId(operator_id));
}

#[tokio::test]
/// Once `operatorId` is decoded, malformed operator payloads must still preserve
/// `max_operator_id_seen` so later valid operator ids are not blocked forever.
async fn test_malformed_operator_still_advances_max_seen() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();

    let mut first_log = create_operator_added_log(
        1,
        Address::random(),
        create_valid_rsa_public_key_bytes(),
        1000,
    );
    first_log.log_index = Some(0);

    let mut malformed_log =
        create_operator_added_log(2, Address::random(), "not-base64".into(), 1000);
    malformed_log.log_index = Some(1);

    let mut third_log = create_operator_added_log(
        3,
        Address::random(),
        create_valid_rsa_public_key_bytes(),
        1000,
    );
    third_log.log_index = Some(2);

    test.processor
        .process_logs(vec![first_log, malformed_log, third_log], true, 12350)
        .expect("Malformed operator should be skipped without blocking later operator ids");

    verify_operator_stored(&test.processor, OperatorId(1));
    verify_operator_stored(&test.processor, OperatorId(3));
    assert!(!test.processor.db.state().operator_exists(&OperatorId(2)));
    assert_eq!(
        test.processor.db.state().get_max_operator_id_seen(),
        Some(3)
    );
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12350);
}

#[tokio::test]
/// Duplicate operator public keys are handled like malformed history: skip the bad event, preserve
/// `max_operator_id_seen`, and continue applying later valid operator ids.
async fn test_duplicate_operator_pubkey_is_skipped_without_blocking_later_ids() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();
    let duplicate_public_key = create_valid_rsa_public_key_bytes();

    let mut first_log =
        create_operator_added_log(1, Address::random(), duplicate_public_key.clone(), 1000);
    first_log.log_index = Some(0);

    let mut duplicate_log =
        create_operator_added_log(2, Address::random(), duplicate_public_key, 1000);
    duplicate_log.log_index = Some(1);

    let mut third_log = create_operator_added_log(
        3,
        Address::random(),
        create_valid_rsa_public_key_bytes(),
        1000,
    );
    third_log.log_index = Some(2);

    test.processor
        .process_logs(vec![first_log, duplicate_log, third_log], true, 12351)
        .expect("Duplicate operator pubkey should be skipped without blocking later operator ids");

    verify_operator_stored(&test.processor, OperatorId(1));
    verify_operator_stored(&test.processor, OperatorId(3));
    assert!(!test.processor.db.state().operator_exists(&OperatorId(2)));
    assert_eq!(
        test.processor.db.state().get_max_operator_id_seen(),
        Some(3)
    );
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12351);
}

#[tokio::test]
/// Re-fetching a block after one log in that block was already committed should skip the committed
/// log and continue from the next log instead of replaying it.
async fn test_resume_skips_already_processed_operator_in_same_block() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();

    let first_operator_id = 1u64;
    let second_operator_id = 2u64;
    let first_owner = Address::random();
    let first_rsa_pubkey = generators::pubkey::random_rsa();
    let first_public_key = base64::engine::general_purpose::STANDARD
        .encode(
            first_rsa_pubkey
                .public_key_to_pem()
                .expect("Failed to encode RSA public key"),
        )
        .into_bytes()
        .into();

    let first_log =
        create_operator_added_log(first_operator_id, first_owner, first_public_key, 1000);
    let mut second_log = create_operator_added_log(
        second_operator_id,
        Address::random(),
        create_valid_rsa_public_key_bytes(),
        1000,
    );
    second_log.log_index = Some(1);

    let cursor = database::ProcessedEventCursor {
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

    test.processor
        .process_logs(vec![first_log, second_log], true, 12345)
        .expect("Processing resumed logs should succeed");

    verify_operator_stored(&test.processor, OperatorId(first_operator_id));
    verify_operator_stored(&test.processor, OperatorId(second_operator_id));
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12345);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

#[tokio::test]
/// KeySplit mode still processes `OperatorAdded` fully because operator state is needed there too.
async fn test_keysplit_mode_processing() {
    use database::test_utils::TEST_NETWORK;

    setup_tracing();

    let pubkey = generators::pubkey::random_rsa();
    let db = Arc::new(
        database::NetworkDatabase::new_in_memory(&pubkey, TEST_NETWORK)
            .expect("Failed to create in-memory database"),
    );
    let processor = create_keysplit_mode_processor(db);

    let operator_id = 1u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    let log = create_operator_added_log(operator_id, owner, public_key, 1500);

    let result = processor.process_logs(vec![log], true, 12352);
    assert!(
        result.is_ok(),
        "KeySplit mode should process OperatorAdded events"
    );

    verify_operator_stored(&processor, OperatorId(operator_id));
}
