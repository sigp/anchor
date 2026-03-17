//! Focused `ValidatorRemoved` coverage for missing-state behavior.

use alloy::primitives::Bytes;

mod common;

use common::*;

#[tokio::test]
/// A valid `ValidatorRemoved` event should delete the validator, its shares, and the now-empty
/// cluster state, then finish on the processed-block boundary.
async fn test_validator_removed_event_cleans_state_and_advances_progress() {
    setup_tracing();

    // Arrange: start from the seeded validator/cluster fixture.
    let test = ProcessorFixture::new();
    let owner = test.cluster.owner;
    let operator_ids = test
        .cluster
        .cluster_members
        .iter()
        .map(|operator_id| **operator_id)
        .collect();
    let public_key = Bytes::from(test.validator.public_key.serialize().to_vec());
    let log = create_validator_removed_log(owner, operator_ids, public_key);

    // Act: process the removal event.
    test.processor
        .process_logs(vec![log], true, 12369)
        .expect("ValidatorRemoved should commit successfully");

    // Assert: the validator, shares, and cluster were all removed, and progress advanced.
    assert_eq!(test.processor.db.state().metadata().length(), 0);
    assert_eq!(test.processor.db.state().shares().length(), 0);
    assert_eq!(test.processor.db.state().clusters().length(), 0);
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12369);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

#[tokio::test]
/// `ValidatorRemoved` still treats missing committed validator state as a fatal inconsistency.
/// This protects the branch from silently skipping over unexpected DB/state divergence.
async fn test_validator_removed_missing_state_is_fatal() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();
    let owner = test.cluster.owner;
    let operator_ids = vec![1u64, 2, 3, 4];
    let public_key = Bytes::from(test.validator.public_key.serialize().to_vec());
    let log = create_validator_removed_log(owner, operator_ids, public_key);

    let result = test.processor.process_logs(vec![log], true, 12370);
    assert!(
        result.is_err(),
        "Missing committed validator state should remain a fatal inconsistency"
    );

    assert_eq!(test.processor.db.state().get_last_processed_block(), 0);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

#[tokio::test]
/// Owner mismatches are malformed event data, not local DB corruption. The validator should stay
/// intact, but progress must still advance because the bad event was intentionally skipped.
async fn test_validator_removed_owner_mismatch_is_skipped_without_mutating_state() {
    setup_tracing();

    // Arrange: use a valid seeded validator but forge the log with a different owner.
    let test = ProcessorFixture::new();
    let operator_ids = test
        .cluster
        .cluster_members
        .iter()
        .map(|operator_id| **operator_id)
        .collect();
    let public_key = Bytes::from(test.validator.public_key.serialize().to_vec());
    let log = create_validator_removed_log(
        alloy::primitives::Address::random(),
        operator_ids,
        public_key,
    );

    // Act: process the malformed removal event.
    test.processor
        .process_logs(vec![log], true, 12371)
        .expect("Owner mismatches should be skipped without aborting processing");

    // Assert: the validator remains present, but progress advanced past the skipped event.
    assert_eq!(test.processor.db.state().metadata().length(), 1);
    assert_eq!(test.processor.db.state().clusters().length(), 1);
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12371);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}
