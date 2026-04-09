//! Integration tests for committee aggregate signing in `sign_aggregate_and_proofs()`.
use std::time::Duration;

use futures::StreamExt;
use signature_collector::SignatureRequester;
use ssv_types::OperatorId;
use types::{MainnetEthSpec, SignedAggregateAndProof};
use validator_store::ValidatorStore;

use super::common::*;
use crate::Error;

type SignAggregatesResult = Vec<Result<Vec<SignedAggregateAndProof<MainnetEthSpec>>, Error>>;

const PRIMARY_COMMITTEE_OPERATOR_IDS: [OperatorId; 4] =
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
const SECONDARY_COMMITTEE_OPERATOR_IDS: [OperatorId; 4] =
    [OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)];

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
const PRIMARY_COMMITTEE_INDEX: usize = 0;
const SECONDARY_COMMITTEE_INDEX: usize = 1;
const FIRST_VALIDATOR_INDEX: usize = 0;
const SECOND_VALIDATOR_INDEX: usize = 1;

const PRIMARY_COMMITTEE_VALIDATOR_COUNT: usize = 2;
const SINGLE_VALIDATOR_COUNT: usize = 1;
const PRIMARY_COMMITTEE_STARTING_VALIDATOR_INDEX: usize = 0;
const SECONDARY_COMMITTEE_STARTING_VALIDATOR_INDEX: usize = 100;

const NEXT_SLOT: u64 = TEST_SLOT + 1;
const STREAM_ITEM_TIMEOUT: Duration = Duration::from_secs(5);
const BLOCKED_STREAM_TIMEOUT: Duration = Duration::from_millis(500);

/// `sign_aggregate_and_proofs` with empty input yields zero stream items.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_empty_input() {
    let committee = create_primary_committee_setup(SINGLE_VALIDATOR_COUNT);
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
/// committee, collects one signature per validator, and streams one batch per committee.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_produces_one_stream_item_per_committee() {
    // Arrange
    let committee_a = create_primary_committee_setup(PRIMARY_COMMITTEE_VALIDATOR_COUNT);
    let committee_b = create_secondary_committee_setup(SINGLE_VALIDATOR_COUNT);
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
    let expected_total = PRIMARY_COMMITTEE_VALIDATOR_COUNT + SINGLE_VALIDATOR_COUNT;
    assert_eq!(
        total, expected_total,
        "expected {expected_total} total signed aggregates"
    );

    // We make one `sign_and_collect` call per validator.
    // The committee requester stores the local batch size for that validator's committee:
    // committee A has 2 validators, so it produces 2 calls, each with batch size 2;
    // committee B has 1 validator, so it produces 1 call with batch size 1.
    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        expected_total,
        "expected {expected_total} sign_and_collect calls"
    );
    let batch_sizes: Vec<_> = captured
        .iter()
        .map(|call| match &call.requester {
            SignatureRequester::Committee {
                validator_partial_signature_batch_size,
                ..
            } => *validator_partial_signature_batch_size,
            other => panic!("expected SignatureRequester::Committee, got: {other:?}"),
        })
        .collect();

    let calls_with_batch_size = |batch_size: usize| {
        batch_sizes
            .iter()
            .filter(|&&size| size == batch_size)
            .count()
    };
    assert_eq!(
        calls_with_batch_size(PRIMARY_COMMITTEE_VALIDATOR_COUNT),
        PRIMARY_COMMITTEE_VALIDATOR_COUNT,
        "committee A should make one call per validator, and each call should have batch size {}",
        PRIMARY_COMMITTEE_VALIDATOR_COUNT,
    );
    assert_eq!(
        calls_with_batch_size(SINGLE_VALIDATOR_COUNT),
        SINGLE_VALIDATOR_COUNT,
        "committee B should make one call for its only validator, and that call should have batch size {}",
        SINGLE_VALIDATOR_COUNT,
    );
}

/// One committee blocked on missing `AggregationAssignments` must not stop another committee from
/// producing its aggregate batch. This verifies that committee futures stay isolated.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sign_aggregate_and_proofs_failure_isolation() {
    // Arrange
    let committee_a = create_primary_committee_setup(SINGLE_VALIDATOR_COUNT);
    let committee_b = create_secondary_committee_setup(SINGLE_VALIDATOR_COUNT);
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

fn create_primary_committee_setup(num_validators: usize) -> CommitteeSetup {
    create_committee_setup(
        &PRIMARY_COMMITTEE_OPERATOR_IDS,
        num_validators,
        PRIMARY_COMMITTEE_STARTING_VALIDATOR_INDEX,
    )
}

fn create_secondary_committee_setup(num_validators: usize) -> CommitteeSetup {
    create_committee_setup(
        &SECONDARY_COMMITTEE_OPERATOR_IDS,
        num_validators,
        SECONDARY_COMMITTEE_STARTING_VALIDATOR_INDEX,
    )
}
