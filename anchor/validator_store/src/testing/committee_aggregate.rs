//! Integration tests for the committee-batching aggregate path in `sign_aggregate_and_proofs()`.
use std::time::Duration;

use futures::StreamExt;
use signature_collector::SignatureRequester;
use ssv_types::OperatorId;
use types::{MainnetEthSpec, SignedAggregateAndProof};
use validator_store::ValidatorStore;

use super::common::*;
use crate::Error;

type SignAggregatesResult = Vec<Result<Vec<SignedAggregateAndProof<MainnetEthSpec>>, Error>>;

/// `sign_aggregate_and_proofs` groups aggregates by `CommitteeId`, runs consensus once per
/// committee, collects committee signatures for each validator, and streams one batch per
/// committee.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_produces_one_stream_item_per_committee() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee_a = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        2,
        0,
    );
    let committee_b = create_committee_setup(
        &[OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)],
        1,
        100,
    );
    assert_ne!(
        committee_a.cluster.committee_id(),
        committee_b.cluster.committee_id(),
    );
    let harness = ValidatorStoreTestHarness::new(vec![committee_a, committee_b], our_operator_id);
    harness.seed_aggregation_assignments_for_slot(TEST_SLOT, &[0, 1]);
    let aggregates = vec![
        harness.create_aggregate(0, 0),
        harness.create_aggregate(0, 1),
        harness.create_aggregate(1, 0),
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
    assert_eq!(total, 3, "expected 3 total signed aggregates");

    let captured = harness.captured_calls.lock();
    assert_eq!(captured.len(), 3, "expected 3 sign_and_collect calls");
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
    assert_eq!(
        requested_counts,
        vec![1, 2, 2],
        "expected committee requester counts to match aggregators per committee"
    );
}

/// A committee blocked waiting for `AggregationAssignments` does not prevent another committee
/// from producing its aggregate batch. Verifies the committee futures are isolated.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_failure_isolation() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee_a = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        1,
        0,
    );
    let committee_b = create_committee_setup(
        &[OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)],
        1,
        100,
    );
    let harness = ValidatorStoreTestHarness::new(vec![committee_a, committee_b], our_operator_id);
    harness.seed_aggregation_assignments_for_slot(TEST_SLOT, &[0]);
    let aggregates = vec![
        harness.create_aggregate(0, 0),
        harness.create_aggregate_at_slot(1, 0, TEST_SLOT + 1),
    ];

    // Act
    let stream = harness
        .validator_store
        .sign_aggregate_and_proofs(aggregates);
    tokio::pin!(stream);
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
    let second = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;

    // Assert
    let first_item = first
        .expect("first committee should complete within timeout")
        .expect("stream should yield an item");
    assert!(first_item.is_ok());
    assert!(
        second.is_err(),
        "stuck committee should not produce a result"
    );
}
