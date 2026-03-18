//! Integration tests for the committee-batching attestation path in `sign_attestations()`.

use std::time::Duration;

use futures::StreamExt;
use signature_collector::SignatureRequester;
use ssv_types::{OperatorId, msgid::Role, partial_sig::PartialSignatureKind};
use types::{Attestation, Hash256, MainnetEthSpec, Slot};
use validator_store::ValidatorStore;

use super::common::*;
use crate::Error;

type SignAttestationsResult = Vec<Result<Vec<(u64, Attestation<MainnetEthSpec>)>, Error>>;

/// `sign_attestations` groups attestations by `CommitteeId` and produces one stream item per
/// committee. With a single operator, QBFT times out for each committee, but the stream should
/// still yield one `Ok(empty)` per committee (errors are caught internally).
#[tokio::test(flavor = "multi_thread")]
async fn sign_attestations_produces_one_stream_item_per_committee() {
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
    harness.seed_voting_context();
    harness.set_clock_for_instant_timeout();

    let attestations = vec![
        harness.create_attestation(0, 0),
        harness.create_attestation(0, 1),
        harness.create_attestation(1, 0),
    ];

    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    let mut results: SignAttestationsResult = Vec::new();
    let collected = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(item) = stream.next().await {
            results.push(item);
        }
    })
    .await;

    assert!(collected.is_ok(), "stream should complete within timeout");
    assert_eq!(results.len(), 2, "expected one stream item per committee");
    for result in &results {
        assert!(result.is_ok(), "each item should be Ok (errors are caught)");
    }
}

/// A committee stuck at `get_voting_context` (no voting context for its slot) does not block
/// another committee from completing. Verifies `FuturesUnordered` independence.
#[tokio::test(flavor = "multi_thread")]
async fn sign_attestations_failure_isolation() {
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
    harness.set_clock_for_instant_timeout();

    let attestations = vec![
        harness.create_attestation(0, 0), // committee A, TEST_SLOT
        harness.create_attestation_at_slot(1, 0, TEST_SLOT + 1), // committee B, different slot
    ];

    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    // Committee A should complete (QBFT timeout -> Ok(empty)), committee B should block.
    let first = tokio::time::timeout(Duration::from_secs(30), stream.next()).await;
    assert!(
        first.is_ok(),
        "first committee should complete despite second being stuck"
    );
    let first_item = first.unwrap().expect("stream should yield an item");
    assert!(first_item.is_ok());

    // Second item should NOT arrive (committee B is stuck at get_voting_context)
    let second = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
    assert!(
        second.is_err(),
        "stuck committee should not produce a result"
    );
}

/// `sign_attestations` returns an immediate `NotSynced` error when not synced.
#[tokio::test(flavor = "multi_thread")]
async fn sign_attestations_not_synced() {
    let our_operator_id = OperatorId(1);

    let committee = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        1,
        0,
    );

    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);
    harness.is_synced_tx.send_replace(false);

    let attestations = vec![harness.create_attestation(0, 0)];

    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    let item = stream.next().await.expect("stream should yield one item");
    assert!(
        matches!(
            &item,
            Err(Error::SpecificError(crate::SpecificError::NotSynced))
        ),
        "expected NotSynced error, got: {item:?}"
    );
}

/// `collect_committee_signatures` passes `SignatureRequester::Committee` (not `SingleValidator`)
/// to the signature collector for each validator in the committee.
#[tokio::test(flavor = "multi_thread")]
async fn collect_committee_signatures_uses_committee_mode() {
    let our_operator_id = OperatorId(1);

    let committee = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        3, // 3 validators in this committee
        0,
    );

    let cluster = committee.cluster.clone();
    let validators: Vec<_> = committee
        .validators
        .iter()
        .map(|v| (v.clone(), Hash256::random()))
        .collect();

    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);

    let base_hash = Hash256::random();
    let num_sigs = 5;

    let result = harness
        .validator_store
        .collect_committee_signatures(
            PartialSignatureKind::PostConsensus,
            Role::Committee,
            Slot::new(TEST_SLOT),
            &cluster,
            num_sigs,
            base_hash,
            validators,
        )
        .await;

    assert!(
        result.is_ok(),
        "collect_committee_signatures should succeed"
    );
    let signatures = result.unwrap();
    assert_eq!(
        signatures.len(),
        3,
        "should have one signature per validator"
    );

    let captured = harness.captured_calls.lock();
    assert_eq!(captured.len(), 3, "mock should have been called 3 times");

    for call in captured.iter() {
        match &call.requester {
            SignatureRequester::Committee {
                num_signatures_to_collect,
                base_hash: captured_hash,
            } => {
                assert_eq!(*num_signatures_to_collect, num_sigs);
                assert_eq!(*captured_hash, base_hash);
            }
            other => panic!("expected SignatureRequester::Committee, got: {other:?}"),
        }
    }
}
