//! Integration tests for `sign_aggregate_and_proofs()`.
//!
//! At Boole+ this Lighthouse callback is deliberately inert: Anchor is the authoritative publisher
//! of committee aggregates, signing and posting the decided worklist itself (see
//! [`crate::aggregator_post_consensus`]). Anything the callback returned would be published a
//! second time by Lighthouse, so it hands back an empty batch. Pre-Boole is unchanged: the callback
//! still signs per validator and Lighthouse still publishes the result.

use std::collections::HashSet;

use fork::Fork;
use signature_collector::SignatureRequester;
use ssv_types::{OperatorId, msgid::Role};

use super::common::*;

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
const PRIMARY_COMMITTEE_INDEX: usize = 0;
const SECONDARY_COMMITTEE_INDEX: usize = 1;
const FIRST_VALIDATOR_INDEX: usize = 0;
const SECOND_VALIDATOR_INDEX: usize = 1;

const PRIMARY_COMMITTEE_VALIDATOR_COUNT: usize = 2;
const SINGLE_VALIDATOR_COUNT: usize = 1;

// ==================== Boole+ tests ====================

/// Empty input yields the same single empty batch as any other Boole+ call: there is no fork to
/// determine and nothing to publish either way.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_empty_input_yields_one_empty_batch() {
    // Arrange
    let committee = create_primary_committee_setup(SINGLE_VALIDATOR_COUNT);
    let harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR_ID);

    // Act
    let results = harness.collect_aggregates(vec![]).await;

    // Assert
    assert_single_empty_batch(results, "aggregates");
}

/// Boole+ yields one empty batch however many committees the request spans, without waiting on
/// `AggregationAssignments` and without collecting any signature of its own.
///
/// The single item is the stream's completion signal, not a per-committee result: the callback no
/// longer groups by committee, because it no longer produces anything Lighthouse could publish. It
/// also no longer parks on the assignments watch channel, which is why nothing is seeded here.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_boole_yields_one_empty_batch_for_all_committees() {
    // Arrange: two distinct committees, no assignments published for the slot at all.
    let committee_a = create_primary_committee_setup(PRIMARY_COMMITTEE_VALIDATOR_COUNT);
    let committee_b = create_secondary_committee_setup(SINGLE_VALIDATOR_COUNT);
    assert_ne!(
        committee_a.cluster.committee_id(),
        committee_b.cluster.committee_id(),
    );
    let harness = ValidatorStoreTestHarness::new(vec![committee_a, committee_b], OUR_OPERATOR_ID);
    let aggregates = vec![
        harness.create_aggregate(PRIMARY_COMMITTEE_INDEX, FIRST_VALIDATOR_INDEX),
        harness.create_aggregate(PRIMARY_COMMITTEE_INDEX, SECOND_VALIDATOR_INDEX),
        harness.create_aggregate(SECONDARY_COMMITTEE_INDEX, FIRST_VALIDATOR_INDEX),
    ];

    // Act
    let results = harness.collect_aggregates(aggregates).await;

    // Assert
    assert_single_empty_batch(results, "aggregates");
    assert!(
        harness.captured_calls.lock().is_empty(),
        "the callback must not collect signatures; signing belongs to the post-consensus execution"
    );
}

// ==================== Pre-Boole tests ====================

/// Pre-Boole the callback still signs one aggregate per validator and returns them for Lighthouse
/// to publish. The empty-batch contract is Boole+ only.
#[tokio::test(flavor = "multi_thread")]
async fn sign_aggregate_and_proofs_pre_boole_signs_each_validator() {
    // Arrange
    let committee = create_primary_committee_setup(PRIMARY_COMMITTEE_VALIDATOR_COUNT);
    let harness =
        ValidatorStoreTestHarness::new_with_fork(vec![committee], OUR_OPERATOR_ID, Fork::Alan);
    let aggregates = vec![
        harness.create_aggregate(PRIMARY_COMMITTEE_INDEX, FIRST_VALIDATOR_INDEX),
        harness.create_aggregate(PRIMARY_COMMITTEE_INDEX, SECOND_VALIDATOR_INDEX),
    ];

    // Act
    let results = harness.collect_aggregates(aggregates).await;

    // Assert: one batch holding both validators' signed aggregates.
    assert_eq!(results.len(), 1, "the pre-Boole path yields one batch");
    let signed = results
        .into_iter()
        .next()
        .expect("stream item should exist")
        .expect("pre-Boole signing should succeed");
    let signed_aggregators: HashSet<u64> = signed
        .iter()
        .map(|signed| signed.message().aggregator_index())
        .collect();
    let expected_aggregators: HashSet<u64> = [FIRST_VALIDATOR_INDEX, SECOND_VALIDATOR_INDEX]
        .iter()
        .map(|&position| harness.aggregator_index(PRIMARY_COMMITTEE_INDEX, position))
        .collect();
    assert_eq!(
        signed_aggregators, expected_aggregators,
        "every requested validator should get a signed aggregate back"
    );

    // Signature collection is per validator, not batched into one committee message.
    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        PRIMARY_COMMITTEE_VALIDATOR_COUNT,
        "expected one sign_and_collect call per validator"
    );
    for call in captured.iter() {
        assert_eq!(call.metadata.role, Role::Aggregator);
        assert!(
            matches!(call.requester, SignatureRequester::SingleValidator { .. }),
            "pre-Boole aggregates are collected per validator, not as a committee batch"
        );
    }
}
