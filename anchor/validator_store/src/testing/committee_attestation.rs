//! Integration tests for the committee-batching attestation path in `sign_attestations()`.
use std::time::Duration;

use futures::StreamExt;
use signature_collector::SignatureRequester;
use ssv_types::OperatorId;
use types::{Attestation, MainnetEthSpec};
use validator_store::ValidatorStore;

use super::common::*;
use crate::Error;

type SignAttestationsResult = Vec<Result<Vec<(u64, Attestation<MainnetEthSpec>)>, Error>>;

/// `sign_attestations` groups attestations by `CommitteeId`, runs consensus once per committee,
/// collects signatures for each validator, and streams one batch of signed attestations per
/// committee.
#[tokio::test(flavor = "multi_thread")]
async fn sign_attestations_produces_one_stream_item_per_committee() {
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
    // Precondition: committees must have distinct IDs for the test to be meaningful
    assert_ne!(
        committee_a.cluster.committee_id(),
        committee_b.cluster.committee_id(),
    );
    let harness = ValidatorStoreTestHarness::new(vec![committee_a, committee_b], our_operator_id);
    harness.seed_voting_context();
    let attestations = vec![
        harness.create_attestation(0, 0),
        harness.create_attestation(0, 1),
        harness.create_attestation(1, 0),
    ];

    // Act
    let results: SignAttestationsResult = harness
        .validator_store
        .sign_attestations(attestations)
        .collect()
        .await;

    // Assert — 2 stream items (one per committee), 3 total signed attestations
    assert_eq!(results.len(), 2, "expected one stream item per committee");
    let all_signed: Vec<_> = results
        .into_iter()
        .map(|r| r.expect("each committee batch should succeed"))
        .collect();
    let total: usize = all_signed.iter().map(|batch| batch.len()).sum();
    assert_eq!(total, 3, "expected 3 total signed attestations");

    // Verify the mock signature collector was called via the committee path with the expected
    // per-committee request counts. In this harness all validators are attesters and there are no
    // sync committee duties, so the expected counts are the number of validators per committee.
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
        "expected committee requester counts to match validators per committee"
    );
}

/// A committee stuck at `get_voting_context` (no voting context for its slot) does not block
/// another committee from signing successfully. Verifies `FuturesUnordered` independence.
#[tokio::test(flavor = "multi_thread")]
async fn sign_attestations_failure_isolation() {
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
    // Only seed voting context for TEST_SLOT — committee B's attestation uses a different
    // slot, so its `get_voting_context` will block indefinitely.
    harness.seed_voting_context();
    let attestations = vec![
        harness.create_attestation(0, 0), // committee A, TEST_SLOT
        harness.create_attestation_at_slot(1, 0, TEST_SLOT + 1), // committee B, different slot
    ];

    // Act
    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
    let second = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;

    // Assert — committee A completes, committee B stays stuck
    let first_item = first
        .expect("first committee should complete within timeout")
        .expect("stream should yield an item");
    assert!(first_item.is_ok());
    assert!(
        second.is_err(),
        "stuck committee should not produce a result"
    );
}

/// `sign_attestations` returns an immediate `NotSynced` error when not synced.
#[tokio::test(flavor = "multi_thread")]
async fn sign_attestations_not_synced() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        1,
        0,
    );
    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);
    harness.is_synced_tx.send_replace(false);
    let attestations = vec![harness.create_attestation(0, 0)];

    // Act
    let results: SignAttestationsResult = harness
        .validator_store
        .sign_attestations(attestations)
        .collect()
        .await;

    // Assert
    assert_eq!(results.len(), 1, "stream should yield exactly one item");
    assert!(
        matches!(
            &results[0],
            Err(Error::SpecificError(crate::SpecificError::NotSynced))
        ),
        "expected NotSynced error, got: {:?}",
        results[0]
    );
}
