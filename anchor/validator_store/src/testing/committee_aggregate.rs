//! Integration tests for the committee-batching aggregate path in `sign_aggregate_and_proofs()`.
use futures::StreamExt;
use signature_collector::SignatureRequester;
use types::{MainnetEthSpec, SignedAggregateAndProof};
use validator_store::ValidatorStore;

use super::common::*;
use crate::Error;

type SignAggregatesResult = Vec<Result<Vec<SignedAggregateAndProof<MainnetEthSpec>>, Error>>;

const PRIMARY_COMMITTEE_VALIDATOR_COUNT: usize = 2;
const SINGLE_VALIDATOR_COMMITTEE_COUNT: usize = 1;
const EXPECTED_TOTAL_SIGNED_AGGREGATES: usize = 3;
const EXPECTED_SIGN_AND_COLLECT_CALLS: usize = 3;
const EXPECTED_REQUESTED_COUNTS: [usize; 3] = [1, 2, 2];

/// `sign_aggregate_and_proofs` with empty input yields zero stream items.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_empty_input() {
    let committee = create_primary_committee_setup(SINGLE_VALIDATOR_COMMITTEE_COUNT);
    let harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR_ID);

    let results: SignAggregatesResult = harness
        .validator_store
        .sign_aggregate_and_proofs(vec![])
        .collect()
        .await;

    assert!(
        results.is_empty(),
        "empty input should yield zero stream items"
    );
}

/// `sign_aggregate_and_proofs` groups aggregates by `CommitteeId`, runs consensus once per
/// committee, collects committee signatures for each validator, and streams one batch per
/// committee.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_produces_one_stream_item_per_committee() {
    // Arrange
    let committee_a = create_primary_committee_setup(PRIMARY_COMMITTEE_VALIDATOR_COUNT);
    let committee_b = create_secondary_committee_setup(SINGLE_VALIDATOR_COMMITTEE_COUNT);
    assert_ne!(
        committee_a.cluster.committee_id(),
        committee_b.cluster.committee_id(),
    );
    let harness = ValidatorStoreTestHarness::new(vec![committee_a, committee_b], OUR_OPERATOR_ID);
    // Seed aggregate assignments for both committees at TEST_SLOT so both committee batches can
    // resolve their decided aggregate data.
    harness.seed_aggregation_assignments_for_slot(
        TEST_SLOT,
        &[PRIMARY_COMMITTEE_INDEX, SECONDARY_COMMITTEE_INDEX],
    );
    let aggregates = vec![
        harness.create_aggregate(PRIMARY_COMMITTEE_INDEX, FIRST_VALIDATOR_INDEX),
        harness.create_aggregate(PRIMARY_COMMITTEE_INDEX, SECOND_VALIDATOR_INDEX),
        harness.create_aggregate(SECONDARY_COMMITTEE_INDEX, FIRST_VALIDATOR_INDEX),
    ];

    // Act
    let results: SignAggregatesResult = harness
        .validator_store
        .sign_aggregate_and_proofs(aggregates)
        .collect()
        .await;

    // Assert
    assert_eq!(results.len(), 2, "expected one stream item per committee");
    let all_signed: Vec<Vec<SignedAggregateAndProof<MainnetEthSpec>>> = results
        .into_iter()
        .map(|r| r.expect("each committee batch should succeed"))
        .collect();
    let total: usize = all_signed.iter().map(|batch| batch.len()).sum();
    assert_eq!(
        total, EXPECTED_TOTAL_SIGNED_AGGREGATES,
        "expected 3 total signed aggregates"
    );

    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        EXPECTED_SIGN_AND_COLLECT_CALLS,
        "expected 3 sign_and_collect calls"
    );
    let mut requested_counts: Vec<_> = captured
        .iter()
        .map(|call| match &call.requester {
            SignatureRequester::Committee {
                num_signatures_to_collect,
                ..
            } => *num_signatures_to_collect,
            other => panic!("expected SignatureRequester::Committee, got: {other:?}"),
        })
        .collect();
    requested_counts.sort_unstable();
    // Committee A has two aggregators and committee B has one, so the per-validator collection
    // requests should be [1, 2, 2] irrespective of stream ordering.
    assert_eq!(
        requested_counts, EXPECTED_REQUESTED_COUNTS,
        "expected committee requester counts to match aggregators per committee"
    );
}

/// A committee blocked waiting for `AggregationAssignments` does not prevent another committee
/// from producing its aggregate batch. Verifies the committee futures are isolated.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sign_aggregate_and_proofs_failure_isolation() {
    // Arrange
    let committee_a = create_primary_committee_setup(SINGLE_VALIDATOR_COMMITTEE_COUNT);
    let committee_b = create_secondary_committee_setup(SINGLE_VALIDATOR_COMMITTEE_COUNT);
    let harness = ValidatorStoreTestHarness::new(vec![committee_a, committee_b], OUR_OPERATOR_ID);
    // Only the primary committee gets aggregate assignments for TEST_SLOT.
    harness.seed_aggregation_assignments_for_slot(TEST_SLOT, &[PRIMARY_COMMITTEE_INDEX]);
    let aggregates = vec![
        // Committee A uses TEST_SLOT and should complete.
        harness.create_aggregate(PRIMARY_COMMITTEE_INDEX, FIRST_VALIDATOR_INDEX),
        // Committee B uses NEXT_SLOT, where no aggregate assignments were seeded, so it should
        // stay blocked in the aggregate-assignment lookup.
        harness.create_aggregate_at_slot(
            SECONDARY_COMMITTEE_INDEX,
            FIRST_VALIDATOR_INDEX,
            NEXT_SLOT,
        ),
    ];

    // Act
    let stream = harness
        .validator_store
        .sign_aggregate_and_proofs(aggregates);
    tokio::pin!(stream);
    let first = tokio::time::timeout(STREAM_ITEM_TIMEOUT, stream.next()).await;
    let second = tokio::time::timeout(BLOCKED_STREAM_TIMEOUT, stream.next()).await;

    // Assert
    let first_item = first
        .expect("first committee should complete within timeout")
        .expect("stream should yield an item");
    let signed = first_item.expect("first committee should succeed");
    assert_eq!(
        signed.len(),
        1,
        "successful committee should produce one signed item"
    );
    assert!(
        second.is_err(),
        "stuck committee should not produce a result"
    );
}
