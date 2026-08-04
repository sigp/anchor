//! Integration tests for the committee attestation path in `sign_attestations()`.
use std::time::Duration;

use futures::StreamExt;
use signature_collector::SignatureRequester;
use ssv_types::OperatorId;
use validator_store::ValidatorStore;

use super::common::*;
use crate::Error;

const PRIMARY_COMMITTEE_VALIDATOR_COUNT: usize = 2;
const SINGLE_VALIDATOR_COMMITTEE_VALIDATOR_COUNT: usize = 1;

/// `sign_attestations` groups attestations by `CommitteeId`, runs consensus once per committee,
/// collects one signature per validator, and streams one batch of signed attestations per
/// committee.
#[tokio::test(flavor = "multi_thread")]
async fn sign_attestations_produces_one_stream_item_per_committee() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee_a = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        PRIMARY_COMMITTEE_VALIDATOR_COUNT,
        0,
    );
    let committee_b = create_committee_setup(
        &[OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)],
        SINGLE_VALIDATOR_COMMITTEE_VALIDATOR_COUNT,
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

    // Assert: 2 stream items, one per committee, and 3 signed attestations in total.
    assert_eq!(results.len(), 2, "expected one stream item per committee");
    let all_signed: Vec<_> = results
        .into_iter()
        .map(|r| r.expect("each committee batch should succeed"))
        .collect();
    let total: usize = all_signed.iter().map(|batch| batch.len()).sum();
    let expected_total =
        PRIMARY_COMMITTEE_VALIDATOR_COUNT + SINGLE_VALIDATOR_COMMITTEE_VALIDATOR_COUNT;
    assert_eq!(
        total, expected_total,
        "expected {expected_total} total signed attestations"
    );

    // We make one `sign_and_collect` call per validator.
    // The committee requester carries the batch size for that validator's committee:
    // committee A has 2 validators, so it produces 2 calls, each with a batch size of 2;
    // committee B has 1 validator, so it produces 1 call with a batch size of 1.
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
        "committee A should make one call per validator, and each call should batch {} validator partial signatures",
        PRIMARY_COMMITTEE_VALIDATOR_COUNT,
    );
    assert_eq!(
        calls_with_batch_size(SINGLE_VALIDATOR_COMMITTEE_VALIDATOR_COUNT),
        SINGLE_VALIDATOR_COMMITTEE_VALIDATOR_COUNT,
        "committee B should make one call per validator, and that call should batch {} validator partial signature",
        SINGLE_VALIDATOR_COMMITTEE_VALIDATOR_COUNT,
    );
}

/// A committee stuck in `get_voting_context` because its slot has no voting context does not block
/// another committee from signing successfully. This verifies the `FuturesUnordered` isolation.
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

    // Assert: committee A completes, while committee B remains stuck.
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

/// Each returned `SingleAttestation` must echo the duty's identity fields (`attester_index` and
/// `committee_index`) verbatim: the store signs only `data`, and Lighthouse's attestation service
/// publishes the identity fields exactly as returned.
#[tokio::test(flavor = "multi_thread")]
async fn sign_attestations_echoes_duty_identity_fields() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        PRIMARY_COMMITTEE_VALIDATOR_COUNT,
        0,
    );
    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);
    harness.seed_voting_context();
    let attestations = vec![
        harness.create_attestation(0, 0),
        harness.create_attestation(0, 1),
    ];
    let expected_identities: Vec<(u64, u64)> = attestations
        .iter()
        .map(|duty| (duty.attester_index, duty.committee_index))
        .collect();
    // Precondition: the duties carry distinct identity pairs, so echoing is falsifiable.
    assert_ne!(expected_identities[0], expected_identities[1]);

    // Act
    let signed = run_sign_attestations(&harness, attestations).await;

    // Assert: one attestation per validator, each echoing its duty's identity fields.
    assert_eq!(signed.len(), PRIMARY_COMMITTEE_VALIDATOR_COUNT);
    for (attester_index, committee_index) in expected_identities {
        assert!(
            signed.iter().any(|att| att.attester_index == attester_index
                && att.committee_index == committee_index),
            "expected an attestation echoing attester_index {attester_index} and \
             committee_index {committee_index}"
        );
    }
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
