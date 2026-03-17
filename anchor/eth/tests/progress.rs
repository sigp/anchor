//! Focused progress-boundary coverage for the per-event sync model.

use alloy::primitives::Address;

mod common;

use common::*;

#[tokio::test]
/// Multiple successful events in one fetched range should still collapse back to a single
/// processed-block boundary once the whole range succeeds.
async fn test_multiple_events_processing() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();

    let num_operators = 3u64;
    let mut logs = Vec::new();

    for i in 0..num_operators {
        let operator_id = i + 1;
        let owner = Address::random();
        let public_key = create_valid_rsa_public_key_bytes();
        let fee = 100000;

        let log = create_operator_added_log(operator_id, owner, public_key, fee);
        logs.push(log);
    }

    let result = test.processor.process_logs(logs, true, 12350);
    assert!(result.is_ok(), "Processing multiple events should succeed");

    for i in 0..num_operators {
        verify_operator_stored(&test.processor, ssv_types::OperatorId(i + 1));
    }

    assert_eq!(test.processor.db.state().get_last_processed_block(), 12350);
}

#[tokio::test]
/// Historical/live sync must never regress the coarse processed-block boundary if an older
/// `end_block` is observed after newer progress was already committed.
async fn test_older_end_block_does_not_regress_progress() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();

    test.processor
        .process_logs(Vec::new(), true, 12345)
        .expect("Initial empty batch should advance the processed block");
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12345);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);

    test.processor
        .process_logs(Vec::new(), true, 12344)
        .expect("Older empty batch should be ignored without regressing progress");
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12345);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}
