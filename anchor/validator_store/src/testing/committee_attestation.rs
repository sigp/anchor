//! Integration tests for the committee-batching attestation path in `sign_attestations()`.
//!
//! `sign_attestations()` groups attestations by `CommitteeId`, processes each committee
//! concurrently via `FuturesUnordered`, and returns a `Stream` of results. These tests verify
//! the grouping, streaming, and per-committee consensus flow.

use std::time::Duration;

use futures::StreamExt;
use ssv_types::OperatorId;
use types::{Attestation, MainnetEthSpec};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{Error, SpecificError};

type SignAttestationsResult = Vec<Result<Vec<(u64, Attestation<MainnetEthSpec>)>, Error>>;

/// Test 1: Multiple SSV committees in one `sign_attestations()` call produce separate stream
/// batches (one per committee).
///
/// This test verifies the grouping logic in `sign_attestations()`: attestations belonging to
/// different `CommitteeId` values are processed independently, and the returned `Stream` yields
/// one `Result` item per committee. Because we have a single QBFT manager (our operator) without
/// other operators routing messages back, QBFT consensus will time out. The key assertion is
/// that the stream produces exactly as many items as there are distinct committees, confirming
/// the `FuturesUnordered` fan-out logic.
///
/// The QBFT timeout path returns `Completed::TimedOut`, which `sign_committee_attestations`
/// maps to `Err(Timeout)`. However, `sign_attestations` catches that error and returns
/// `Ok(Vec::new())` per committee. So we expect 2 stream items, both `Ok(empty)`.
#[tokio::test(flavor = "multi_thread")]
async fn test_sign_attestations_produces_one_stream_item_per_committee() {
    // Arrange
    let rsa_private_key = generate_rsa_keypair();
    let rsa_pubkey = rsa_public_from_private(&rsa_private_key);

    let our_operator_id = OperatorId(1);

    // Create 2 committees with distinct operator sets to guarantee different `CommitteeId`s.
    // Committee A uses operators [1, 2, 3, 4], Committee B uses operators [1, 5, 6, 7].
    let operator_ids_a: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
    let operator_ids_b: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)];

    let committee_a = create_committee_setup(
        &operator_ids_a,
        2, // 2 validators in committee A
        our_operator_id,
        &rsa_pubkey,
        0, // starting_validator_index
    );
    let committee_b = create_committee_setup(
        &operator_ids_b,
        1, // 1 validator in committee B
        our_operator_id,
        &rsa_pubkey,
        100, // starting_validator_index (offset to avoid collision)
    );

    // Verify the committees have distinct IDs
    let committee_id_a = committee_a.cluster.committee_id();
    let committee_id_b = committee_b.cluster.committee_id();
    assert_ne!(
        committee_id_a, committee_id_b,
        "Test requires two distinct committee IDs"
    );

    let (executor, _signal) = create_test_executor();

    let harness = ValidatorStoreTestHarness::new(
        vec![committee_a, committee_b],
        our_operator_id,
        rsa_private_key,
        executor,
    );

    harness.seed_voting_context();

    // Build attestations: 2 from committee A, 1 from committee B
    let attestations = vec![
        harness.create_attestation_to_sign(0, 0),
        harness.create_attestation_to_sign(0, 1),
        harness.create_attestation_to_sign(1, 0),
    ];

    // Act: call `sign_attestations` and collect stream items
    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    // Collect results with a timeout to avoid hanging if consensus never completes.
    // Each committee runs QBFT `decide_instance`, which will time out since we only have
    // one operator (no quorum). The timeout duration is per-slot (slot_duration / 3 = 4s),
    // plus QBFT round timeouts. We give 60s total for both committees to time out.
    let mut results: SignAttestationsResult = Vec::new();
    let collect_timeout = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(item) = stream.next().await {
            results.push(item);
        }
    })
    .await;

    // Assert
    // The stream should have completed (not timed out at our 60s boundary).
    if collect_timeout.is_err() {
        panic!(
            "Stream collection timed out after 60s. \
             Got {} items before timeout. Expected 2 items (one per committee).",
            results.len()
        );
    }

    assert_eq!(
        results.len(),
        2,
        "sign_attestations should produce exactly one stream item per committee. \
         Got {} items instead of 2.",
        results.len()
    );

    // Both items should be Ok (errors are caught and converted to Ok(Vec::new()) inside
    // sign_attestations). The actual attestations will be empty because QBFT consensus
    // times out without enough operators to form a quorum.
    for (i, result) in results.iter().enumerate() {
        assert!(
            result.is_ok(),
            "Stream item {i} should be Ok, but got: {result:?}"
        );
    }
}

/// Test 2: A stuck committee does not prevent another committee from producing its stream item.
///
/// This verifies failure isolation in the `FuturesUnordered` fan-out. Committee B's attestations
/// use a slot for which no `VotingContext` is seeded, so `get_voting_context` blocks
/// indefinitely. Committee A's attestations use the seeded slot and proceed to QBFT timeout
/// normally. The stream should yield committee A's result without waiting for committee B.
#[tokio::test(flavor = "multi_thread")]
async fn test_stuck_committee_does_not_block_other_committees() {
    // Arrange
    let rsa_private_key = generate_rsa_keypair();
    let rsa_pubkey = rsa_public_from_private(&rsa_private_key);
    let our_operator_id = OperatorId(1);

    let operator_ids_a: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
    let operator_ids_b: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)];

    let committee_a = create_committee_setup(
        &operator_ids_a,
        2,
        our_operator_id,
        &rsa_pubkey,
        0,
    );
    let committee_b = create_committee_setup(
        &operator_ids_b,
        1,
        our_operator_id,
        &rsa_pubkey,
        100,
    );

    let (executor, _signal) = create_test_executor();

    let harness = ValidatorStoreTestHarness::new(
        vec![committee_a, committee_b],
        our_operator_id,
        rsa_private_key,
        executor,
    );

    // Only seed VotingContext for TEST_SLOT. Committee B's attestations will use a
    // different slot, so its `get_voting_context` call will block forever.
    harness.seed_voting_context();

    // Committee A attestations use the seeded slot (TEST_SLOT).
    // Committee B attestation uses a slot with no VotingContext, causing it to hang.
    let unseeded_slot = TEST_SLOT + 999;
    let attestations = vec![
        harness.create_attestation_to_sign(0, 0),
        harness.create_attestation_to_sign(0, 1),
        harness.create_attestation_to_sign_at_slot(1, 0, unseeded_slot),
    ];

    // Act
    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    // Collect with a timeout. Committee A should complete quickly (QBFT timeout).
    // Committee B blocks on VotingContext, so the stream never fully drains. We expect
    // exactly 1 item (committee A) before our timeout.
    let mut results: SignAttestationsResult = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(item) = stream.next().await {
            results.push(item);
        }
    })
    .await;

    // Assert: committee A produced its stream item despite committee B being stuck
    assert!(
        !results.is_empty(),
        "Expected at least 1 stream item from committee A, but got none. \
         Committee B's hang may have blocked the entire stream."
    );

    assert_eq!(
        results.len(),
        1,
        "Expected exactly 1 stream item (committee A). Committee B should still be stuck. \
         Got {} items.",
        results.len()
    );

    assert!(
        results[0].is_ok(),
        "Committee A's stream item should be Ok, but got: {:?}",
        results[0]
    );
}

/// Test 3: When the node is not synced, `sign_attestations` returns an immediate error without
/// entering the committee-batching or QBFT consensus path.
///
/// This verifies the early-exit guard at the top of `sign_attestations`. The stream should
/// yield exactly one `Err(NotSynced)` item regardless of how many committees or validators
/// are involved.
#[tokio::test(flavor = "multi_thread")]
async fn test_not_synced_returns_immediate_error() {
    // Arrange
    let rsa_private_key = generate_rsa_keypair();
    let rsa_pubkey = rsa_public_from_private(&rsa_private_key);
    let our_operator_id = OperatorId(1);

    let operator_ids_a: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
    let operator_ids_b: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)];

    let committee_a = create_committee_setup(
        &operator_ids_a,
        2,
        our_operator_id,
        &rsa_pubkey,
        0,
    );
    let committee_b = create_committee_setup(
        &operator_ids_b,
        1,
        our_operator_id,
        &rsa_pubkey,
        100,
    );

    let (executor, _signal) = create_test_executor();

    let harness = ValidatorStoreTestHarness::new(
        vec![committee_a, committee_b],
        our_operator_id,
        rsa_private_key,
        executor,
    );

    harness.seed_voting_context();

    // Mark node as not synced
    harness.is_synced_tx.send_replace(false);

    let attestations = vec![
        harness.create_attestation_to_sign(0, 0),
        harness.create_attestation_to_sign(0, 1),
        harness.create_attestation_to_sign(1, 0),
    ];

    // Act
    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    let mut results: SignAttestationsResult = Vec::new();
    // This should return instantly (no QBFT involved), so a short timeout is fine.
    let collect_timeout = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(item) = stream.next().await {
            results.push(item);
        }
    })
    .await;

    // Assert
    assert!(
        collect_timeout.is_ok(),
        "Not-synced path should return immediately, not time out"
    );

    assert_eq!(
        results.len(),
        1,
        "Not-synced should produce exactly 1 error item, got {}.",
        results.len()
    );

    let err = results[0]
        .as_ref()
        .expect_err("Stream item should be Err when not synced");

    assert!(
        matches!(err, validator_store::Error::SpecificError(SpecificError::NotSynced)),
        "Error should be NotSynced, but got: {err:?}"
    );
}
