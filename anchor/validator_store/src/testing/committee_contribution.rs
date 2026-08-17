//! Integration tests for `sign_sync_committee_contributions()` at Boole+.
//!
//! At Boole+ this Lighthouse callback is deliberately inert, exactly like the aggregate one:
//! Anchor is the authoritative publisher of committee sync contributions, signing and posting the
//! decided worklist itself (see [`crate::aggregator_post_consensus`]). Anything the callback
//! returned would be published a second time by Lighthouse, so it hands back an empty batch.
//! Pre-Boole is unchanged and covered in `testing/sync_contribution.rs`.

use ssv_types::OperatorId;

use super::common::*;

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
const PRIMARY_COMMITTEE_INDEX: usize = 0;
const SECONDARY_COMMITTEE_INDEX: usize = 1;
const FIRST_VALIDATOR_INDEX: usize = 0;
const SECOND_VALIDATOR_INDEX: usize = 1;

const PRIMARY_COMMITTEE_VALIDATOR_COUNT: usize = 2;
const SINGLE_VALIDATOR_COUNT: usize = 1;

const FIRST_SUBCOMMITTEE: u64 = 0;
const SECOND_SUBCOMMITTEE: u64 = 1;

/// Empty input yields the same single empty batch as any other Boole+ call: there is no fork to
/// determine and nothing to publish either way.
#[tokio::test(flavor = "multi_thread")]
async fn sign_sync_committee_contributions_empty_input_yields_one_empty_batch() {
    // Arrange
    let committee = create_primary_committee_setup(SINGLE_VALIDATOR_COUNT);
    let harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR_ID);

    // Act
    let results = harness.collect_contributions(vec![]).await;

    // Assert
    assert_single_empty_batch(results, "sync contributions");
}

/// Boole+ yields one empty batch however many committees the request spans, without waiting on
/// `AggregationAssignments` and without collecting any signature of its own.
///
/// The single item is the stream's completion signal, not a per-committee result: the callback no
/// longer groups by committee, because it no longer produces anything Lighthouse could publish.
/// It also no longer parks on the assignments watch channel, which is why nothing is seeded here.
#[tokio::test(flavor = "multi_thread")]
async fn sign_sync_committee_contributions_boole_yields_one_empty_batch_for_all_committees() {
    // Arrange: two distinct committees, no assignments published for the slot at all.
    let committee_a = create_primary_committee_setup(PRIMARY_COMMITTEE_VALIDATOR_COUNT);
    let committee_b = create_secondary_committee_setup(SINGLE_VALIDATOR_COUNT);
    assert_ne!(
        committee_a.cluster.committee_id(),
        committee_b.cluster.committee_id(),
    );
    let harness = ValidatorStoreTestHarness::new(vec![committee_a, committee_b], OUR_OPERATOR_ID);
    let contributions = vec![
        harness.create_contribution(
            PRIMARY_COMMITTEE_INDEX,
            FIRST_VALIDATOR_INDEX,
            FIRST_SUBCOMMITTEE,
        ),
        harness.create_contribution(
            PRIMARY_COMMITTEE_INDEX,
            SECOND_VALIDATOR_INDEX,
            SECOND_SUBCOMMITTEE,
        ),
        harness.create_contribution(
            SECONDARY_COMMITTEE_INDEX,
            FIRST_VALIDATOR_INDEX,
            FIRST_SUBCOMMITTEE,
        ),
    ];

    // Act
    let results = harness.collect_contributions(contributions).await;

    // Assert
    assert_single_empty_batch(results, "sync contributions");
    assert!(
        harness.captured_calls.lock().is_empty(),
        "the callback must not collect signatures; signing belongs to the post-consensus execution"
    );
}
