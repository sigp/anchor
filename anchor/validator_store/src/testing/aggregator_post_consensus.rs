//! Integration tests for the Boole+ `AggregatorCommittee` post-consensus execution.
//!
//! One QBFT decision carries both attestation aggregates and sync contributions. The
//! per-`(committee, slot)` execution signs the complete decided worklist regardless of which
//! Lighthouse callbacks fire; the callbacks only filter the shared outcome down to their own
//! requested items. These tests pin the regression from issue #1227: a callback of one class
//! must still produce the partial signatures for the other class, so the shared committee
//! batch never stalls below its expected size.
//!
//! The worklist tasks are detached, so captured `sign_and_collect` calls arrive
//! asynchronously; assertions on capture counts poll with a deadline and then let the
//! captures settle before asserting exact counts.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use bls::{AggregateSignature, FixedBytesExtended, PublicKeyBytes, Signature};
use futures::StreamExt;
use signature_collector::SignatureRequester;
use ssv_types::{
    CommitteeId, OperatorId, ValidatorIndex, ValidatorMetadata,
    consensus::{AggregatorCommitteeConsensusData, AssignedAggregator, DataVersion, QbftData},
    msgid::Role,
    partial_sig::PartialSignatureKind,
};
use ssz::Encode;
use ssz_types::VariableList;
use types::{
    AttestationBase, AttestationData, Checkpoint, Epoch, ForkName, Hash256, MainnetEthSpec,
    SignedAggregateAndProof, SignedContributionAndProof, Slot, SyncCommitteeContribution,
    SyncSelectionProof,
};
use validator_store::{ContributionToSign, ValidatorStore};

use super::common::*;
use crate::{AggregationAssignments, Error};

type SignAggregatesResult = Vec<Result<Vec<SignedAggregateAndProof<MainnetEthSpec>>, Error>>;
type SignContributionsResult = Vec<Result<Vec<SignedContributionAndProof<MainnetEthSpec>>, Error>>;

const COMMITTEE_OPERATOR_IDS: [OperatorId; 4] =
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
const OUR_OPERATOR_ID: OperatorId = OperatorId(1);

const COMMITTEE_INDEX: usize = 0;
const STARTING_VALIDATOR_INDEX: usize = 0;
const AGGREGATE_VALIDATOR_IDX: usize = 0;
const CONTRIBUTOR_VALIDATOR_IDX: usize = 1;
const MIXED_COMMITTEE_VALIDATOR_COUNT: usize = 2;
const SINGLE_VALIDATOR_COUNT: usize = 1;
/// Position of the only validator in single-validator fixtures.
const SOLE_VALIDATOR_IDX: usize = 0;

const BEACON_COMMITTEE_INDEX: u64 = 0;
const CONFLICTING_BEACON_COMMITTEE_INDEX: u64 = 1;
const CONTRIBUTION_SUBCOMMITTEES: [u64; 2] = [0, 1];
const ALL_SUBCOMMITTEES: [u64; 4] = [0, 1, 2, 3];
const MISSING_SUBCOMMITTEE_INDEX: u64 = 2;

/// Decided validator indices with no local metadata, so the worklist filters them out.
const FOREIGN_AGGREGATOR_INDEX: usize = 900;
const FOREIGN_CONTRIBUTOR_INDEX: usize = 901;

/// One aggregate root plus one contribution root per subcommittee in the standard mixed
/// decided value.
const MIXED_WORKLIST_SIZE: usize = 1 + CONTRIBUTION_SUBCOMMITTEES.len();

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(2);
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Extra wait after the expected capture count is reached, to catch spurious extra captures.
const SETTLE_DELAY: Duration = Duration::from_millis(200);
const STREAM_TIMEOUT: Duration = Duration::from_secs(5);

// ==================== Fixture ====================

/// One committee plus the identifiers needed to seed decided values for it.
///
/// The harness owns its `CommitteeSetup`s privately, so the committee ID and validator
/// metadata are captured before handing the setup over.
struct AggregatorCommitteeFixture {
    harness: ValidatorStoreTestHarness,
    committee_id: CommitteeId,
}

impl AggregatorCommitteeFixture {
    fn new(validator_count: usize) -> Self {
        let setup = create_committee_setup(
            &COMMITTEE_OPERATOR_IDS,
            validator_count,
            STARTING_VALIDATOR_INDEX,
        );
        let committee_id = setup.cluster.committee_id();
        let harness = ValidatorStoreTestHarness::new(vec![setup], OUR_OPERATOR_ID);
        Self {
            harness,
            committee_id,
        }
    }

    fn validator_metadata(&self, position: usize) -> ValidatorMetadata {
        self.harness.validator_metadata(COMMITTEE_INDEX, position)
    }

    fn validator_index(&self, position: usize) -> ValidatorIndex {
        self.validator_metadata(position)
            .index
            .expect("test validator should have an index")
    }

    fn pubkey(&self, position: usize) -> PublicKeyBytes {
        self.validator_metadata(position).public_key
    }

    /// Seeds `AggregationAssignments` at `TEST_SLOT` with the given consensus data for this
    /// committee. The mock decider echoes the proposal, so this is also the decided value.
    fn seed_decided_value(
        &self,
        data: AggregatorCommitteeConsensusData<MainnetEthSpec>,
    ) -> Arc<AggregatorCommitteeConsensusData<MainnetEthSpec>> {
        let data = Arc::new(data);
        self.harness
            .validator_store
            .update_aggregation_assignments(AggregationAssignments {
                slot: Slot::new(TEST_SLOT),
                aggregator_committees: HashMap::new(),
                multi_sync_aggregators: HashMap::new(),
                consensus_data_by_ssv_committee: HashMap::from([(
                    self.committee_id,
                    Arc::clone(&data),
                )]),
            });
        data
    }

    /// Seeds `AggregationAssignments` at `TEST_SLOT` without consensus data for any committee, so
    /// no execution is registered and the callbacks fail with `ConsensusDataNotFound`.
    fn seed_assignments_without_consensus_data(&self) {
        self.harness
            .validator_store
            .update_aggregation_assignments(AggregationAssignments {
                slot: Slot::new(TEST_SLOT),
                aggregator_committees: HashMap::new(),
                multi_sync_aggregators: HashMap::new(),
                consensus_data_by_ssv_committee: HashMap::new(),
            });
    }

    /// Seeds the standard mixed decided value: one aggregate for the aggregate validator plus
    /// one contribution per subcommittee in [`CONTRIBUTION_SUBCOMMITTEES`] for the contributor.
    fn seed_mixed_decided_value(&self) -> Arc<AggregatorCommitteeConsensusData<MainnetEthSpec>> {
        let aggregator = self.validator_index(AGGREGATE_VALIDATOR_IDX);
        let contributor = self.validator_index(CONTRIBUTOR_VALIDATOR_IDX);
        self.seed_decided_value(build_decided_data(
            &[(aggregator, BEACON_COMMITTEE_INDEX)],
            &[
                (contributor, CONTRIBUTION_SUBCOMMITTEES[0]),
                (contributor, CONTRIBUTION_SUBCOMMITTEES[1]),
            ],
        ))
    }

    fn create_contribution(
        &self,
        position: usize,
        subcommittee_index: u64,
    ) -> ContributionToSign<MainnetEthSpec> {
        let validator = self.validator_metadata(position);
        ContributionToSign {
            aggregator_index: *validator
                .index
                .expect("test validator should have an index") as u64,
            aggregator_pubkey: validator.public_key,
            contribution: test_contribution(subcommittee_index),
            selection_proof: SyncSelectionProof::from(Signature::empty()),
        }
    }

    async fn collect_contributions(
        &self,
        contributions: Vec<ContributionToSign<MainnetEthSpec>>,
    ) -> SignContributionsResult {
        let stream = self
            .harness
            .validator_store
            .sign_sync_committee_contributions(contributions);
        tokio::time::timeout(STREAM_TIMEOUT, stream.collect())
            .await
            .expect("contribution callback should complete within the stream timeout")
    }

    async fn collect_aggregates(
        &self,
        aggregates: Vec<validator_store::AggregateToSign<MainnetEthSpec>>,
    ) -> SignAggregatesResult {
        let stream = self
            .harness
            .validator_store
            .sign_aggregate_and_proofs(aggregates);
        tokio::time::timeout(STREAM_TIMEOUT, stream.collect())
            .await
            .expect("aggregate callback should complete within the stream timeout")
    }
}

// ==================== Decided value construction ====================

fn assigned(validator_index: ValidatorIndex, committee_index: u64) -> AssignedAggregator {
    AssignedAggregator {
        validator_index,
        selection_proof: Signature::empty(),
        committee_index,
    }
}

/// An aggregate attestation payload at `TEST_SLOT`. Distinct `committee_index` values produce
/// distinct signing roots.
fn test_aggregate_attestation(committee_index: u64) -> AttestationBase<MainnetEthSpec> {
    AttestationBase {
        aggregation_bits: ssz_types::BitList::with_capacity(128).expect("bitlist should be valid"),
        data: AttestationData {
            slot: Slot::new(TEST_SLOT),
            index: committee_index,
            beacon_block_root: Hash256::zero(),
            source: Checkpoint {
                epoch: Epoch::new(0),
                root: Hash256::zero(),
            },
            target: Checkpoint {
                epoch: Epoch::new(0),
                root: Hash256::zero(),
            },
        },
        signature: AggregateSignature::infinity(),
    }
}

/// A sync contribution at `TEST_SLOT`. Distinct `subcommittee_index` values produce distinct
/// signing roots.
fn test_contribution(subcommittee_index: u64) -> SyncCommitteeContribution<MainnetEthSpec> {
    SyncCommitteeContribution {
        slot: Slot::new(TEST_SLOT),
        beacon_block_root: Hash256::zero(),
        subcommittee_index,
        aggregation_bits: Default::default(),
        signature: AggregateSignature::infinity(),
    }
}

/// Builds an `AggregatorCommitteeConsensusData` from decided entries.
///
/// `aggregators` and `contributors` are `(validator_index, committee_index)` pairs; the
/// attestation and contribution payload lists are derived from them (one payload per unique
/// index).
fn build_decided_data(
    aggregators: &[(ValidatorIndex, u64)],
    contributors: &[(ValidatorIndex, u64)],
) -> AggregatorCommitteeConsensusData<MainnetEthSpec> {
    // One payload per unique committee/subcommittee index, in first-seen order.
    let mut aggregate_committee_indexes: Vec<u64> = Vec::new();
    for &(_, committee_index) in aggregators {
        if !aggregate_committee_indexes.contains(&committee_index) {
            aggregate_committee_indexes.push(committee_index);
        }
    }
    let mut contribution_subcommittees: Vec<u64> = Vec::new();
    for &(_, subcommittee_index) in contributors {
        if !contribution_subcommittees.contains(&subcommittee_index) {
            contribution_subcommittees.push(subcommittee_index);
        }
    }
    let aggregators: Vec<_> = aggregators
        .iter()
        .map(|&(validator_index, committee_index)| assigned(validator_index, committee_index))
        .collect();
    let aggregated_attestations: Vec<_> = aggregate_committee_indexes
        .iter()
        .map(|&committee_index| {
            VariableList::new(test_aggregate_attestation(committee_index).as_ssz_bytes())
                .expect("attestation bytes should fit")
        })
        .collect();
    let contributors: Vec<_> = contributors
        .iter()
        .map(|&(validator_index, subcommittee_index)| assigned(validator_index, subcommittee_index))
        .collect();
    let sync_committee_contributions: Vec<_> = contribution_subcommittees
        .iter()
        .map(|&subcommittee_index| test_contribution(subcommittee_index))
        .collect();

    AggregatorCommitteeConsensusData {
        version: DataVersion::from(ForkName::Deneb),
        aggregators: VariableList::new(aggregators).expect("aggregator list should be valid"),
        aggregator_committee_indexes: VariableList::new(aggregate_committee_indexes)
            .expect("committee indexes should be valid"),
        aggregated_attestations: VariableList::new(aggregated_attestations)
            .expect("aggregated attestations should be valid"),
        contributors: VariableList::new(contributors).expect("contributor list should be valid"),
        sync_committee_contributions: VariableList::new(sync_committee_contributions)
            .expect("contributions should be valid"),
    }
}

// ==================== Capture assertions ====================

/// The committee-requester view of one captured `sign_and_collect` call.
struct CapturedCommitteeCall {
    pubkey: PublicKeyBytes,
    signing_root: Hash256,
    batch_size: usize,
    base_hash: Hash256,
}

/// Snapshots the captured calls, asserting every call is an `AggregatorCommittee`
/// post-consensus call with a `Committee` requester.
fn committee_calls(harness: &ValidatorStoreTestHarness) -> Vec<CapturedCommitteeCall> {
    harness
        .captured_calls
        .lock()
        .iter()
        .map(|call| {
            assert_eq!(call.metadata.role, Role::AggregatorCommittee);
            assert_eq!(call.metadata.kind, PartialSignatureKind::PostConsensus);
            assert_eq!(call.metadata.slot, Slot::new(TEST_SLOT));
            match &call.requester {
                SignatureRequester::Committee {
                    validator_partial_signature_batch_size,
                    base_hash,
                } => CapturedCommitteeCall {
                    pubkey: call.validator_pubkey,
                    signing_root: call.signing_root,
                    batch_size: *validator_partial_signature_batch_size,
                    base_hash: *base_hash,
                },
                other => panic!("expected SignatureRequester::Committee, got: {other:?}"),
            }
        })
        .collect()
}

/// Asserts every call shares the expected local batch size and decided-value batch identity.
fn assert_batch_identity(
    calls: &[CapturedCommitteeCall],
    expected_batch_size: usize,
    expected_base_hash: Hash256,
) {
    for call in calls {
        assert_eq!(
            call.batch_size, expected_batch_size,
            "every root should join the same local committee batch"
        );
        assert_eq!(
            call.base_hash, expected_base_hash,
            "every root should carry the decided-value hash as its batch identity"
        );
    }
}

fn distinct_roots(calls: &[CapturedCommitteeCall]) -> HashSet<Hash256> {
    calls.iter().map(|call| call.signing_root).collect()
}

fn calls_for_pubkey(calls: &[CapturedCommitteeCall], pubkey: PublicKeyBytes) -> usize {
    calls.iter().filter(|call| call.pubkey == pubkey).count()
}

/// Polls until at least `expected` calls were captured, panicking after [`CAPTURE_TIMEOUT`].
///
/// The worklist signing tasks are detached, so captures arrive asynchronously relative to the
/// Lighthouse callback futures.
async fn wait_for_captured_calls(harness: &ValidatorStoreTestHarness, expected: usize) {
    let deadline = tokio::time::Instant::now() + CAPTURE_TIMEOUT;
    loop {
        let count = harness.captured_calls.lock().len();
        if count >= expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {expected} captured sign_and_collect calls within {CAPTURE_TIMEOUT:?}, \
             got {count}"
        );
        tokio::time::sleep(CAPTURE_POLL_INTERVAL).await;
    }
}

/// Waits for `expected` captures, then lets stragglers settle and asserts the count is exact.
async fn assert_captured_calls_settle_at(harness: &ValidatorStoreTestHarness, expected: usize) {
    wait_for_captured_calls(harness, expected).await;
    tokio::time::sleep(SETTLE_DELAY).await;
    let count = harness.captured_calls.lock().len();
    assert_eq!(
        count, expected,
        "capture count should settle at exactly {expected}"
    );
}

/// Unwraps a single-committee stream result into its signed batch.
fn single_batch<T>(results: Vec<Result<Vec<T>, Error>>) -> Vec<T> {
    assert_eq!(results.len(), 1, "expected one committee stream item");
    results
        .into_iter()
        .next()
        .expect("committee stream item should exist")
        .expect("committee signing should succeed")
}

// ==================== Complete-worklist tests ====================

/// A contribution-only Lighthouse callback must still trigger signing of the decided
/// aggregate: the detached execution signs the complete mixed worklist, and every root joins
/// one three-entry committee batch keyed by the decided-value hash.
#[tokio::test(flavor = "multi_thread")]
async fn contribution_only_callback_signs_complete_mixed_worklist() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let decided = fixture.seed_mixed_decided_value();
    let contributions = CONTRIBUTION_SUBCOMMITTEES
        .iter()
        .map(|&subcommittee| fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, subcommittee))
        .collect();

    // Act: only the contribution callback fires.
    let results = fixture.collect_contributions(contributions).await;

    // Assert: the callback returns exactly its own two contributions.
    let mut signed = single_batch(results);
    signed.sort_by_key(|item| item.message.contribution.subcommittee_index);
    assert_eq!(
        signed.len(),
        CONTRIBUTION_SUBCOMMITTEES.len(),
        "the contribution callback should return only its requested contributions"
    );
    for (item, &subcommittee) in signed.iter().zip(CONTRIBUTION_SUBCOMMITTEES.iter()) {
        assert_eq!(item.message.contribution.subcommittee_index, subcommittee);
        assert_eq!(
            item.message.aggregator_index,
            *fixture.validator_index(CONTRIBUTOR_VALIDATOR_IDX) as u64
        );
    }

    // The aggregate root is signed by a detached task even though no aggregate callback fired.
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(
        distinct_roots(&calls).len(),
        MIXED_WORKLIST_SIZE,
        "aggregate and per-subcommittee contribution roots must stay distinct"
    );
    assert_batch_identity(&calls, MIXED_WORKLIST_SIZE, decided.hash());
    assert_eq!(
        calls_for_pubkey(&calls, fixture.pubkey(AGGREGATE_VALIDATOR_IDX)),
        1,
        "the decided aggregate must be signed without an aggregate callback"
    );
    assert_eq!(
        calls_for_pubkey(&calls, fixture.pubkey(CONTRIBUTOR_VALIDATOR_IDX)),
        CONTRIBUTION_SUBCOMMITTEES.len(),
        "the contributor must sign one root per subcommittee"
    );
}

/// The mirror case: an aggregate-only callback must still trigger signing of both decided
/// contributions.
#[tokio::test(flavor = "multi_thread")]
async fn aggregate_only_callback_signs_complete_mixed_worklist() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let decided = fixture.seed_mixed_decided_value();
    let aggregates = vec![
        fixture
            .harness
            .create_aggregate(COMMITTEE_INDEX, AGGREGATE_VALIDATOR_IDX),
    ];

    // Act: only the aggregate callback fires.
    let results = fixture.collect_aggregates(aggregates).await;

    // Assert: the callback returns exactly its own aggregate.
    let signed = single_batch(results);
    assert_eq!(
        signed.len(),
        1,
        "the aggregate callback should return only its requested aggregate"
    );

    // Both contribution roots are signed by detached tasks without a contribution callback.
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(distinct_roots(&calls).len(), MIXED_WORKLIST_SIZE);
    assert_batch_identity(&calls, MIXED_WORKLIST_SIZE, decided.hash());
    assert_eq!(
        calls_for_pubkey(&calls, fixture.pubkey(AGGREGATE_VALIDATOR_IDX)),
        1
    );
    assert_eq!(
        calls_for_pubkey(&calls, fixture.pubkey(CONTRIBUTOR_VALIDATOR_IDX)),
        CONTRIBUTION_SUBCOMMITTEES.len(),
        "the decided contributions must be signed without a contribution callback"
    );
}

/// The slot pipeline signs the complete decided worklist with no Lighthouse callback involved.
///
/// This is the property the whole design rests on: what gets signed is a function of the decided
/// value alone, so a class Lighthouse never asks about can no longer leave the committee batch
/// short of its expected size.
#[tokio::test(flavor = "multi_thread")]
async fn decided_worklist_is_signed_without_any_callback() {
    // Arrange and act: publishing the assignments is the entire trigger.
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let decided = fixture.seed_mixed_decided_value();

    // Assert: every decided root of both classes was submitted to one correctly sized batch.
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(
        distinct_roots(&calls).len(),
        MIXED_WORKLIST_SIZE,
        "both object classes must be signed without either callback firing"
    );
    assert_batch_identity(&calls, MIXED_WORKLIST_SIZE, decided.hash());
}

/// Both callbacks read the same execution: neither re-runs consensus nor duplicates signatures,
/// and each returns only its own items.
#[tokio::test(flavor = "multi_thread")]
async fn both_callbacks_share_one_execution() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let decided = fixture.seed_mixed_decided_value();
    let contributions = CONTRIBUTION_SUBCOMMITTEES
        .iter()
        .map(|&subcommittee| fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, subcommittee))
        .collect();
    let aggregates = vec![
        fixture
            .harness
            .create_aggregate(COMMITTEE_INDEX, AGGREGATE_VALIDATOR_IDX),
    ];

    // Act: contribution callback first, then the aggregate callback joins the cached outcome.
    let contribution_results = fixture.collect_contributions(contributions).await;
    let aggregate_results = fixture.collect_aggregates(aggregates).await;

    // Assert: each callback returned only its own items.
    assert_eq!(
        single_batch(contribution_results).len(),
        CONTRIBUTION_SUBCOMMITTEES.len(),
        "the contribution callback should return only its contributions"
    );
    assert_eq!(
        single_batch(aggregate_results).len(),
        1,
        "the aggregate callback should return only its aggregate"
    );

    // The union is signed exactly once: no duplicate captures from the second callback.
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(distinct_roots(&calls).len(), MIXED_WORKLIST_SIZE);
    assert_batch_identity(&calls, MIXED_WORKLIST_SIZE, decided.hash());
}

/// Dropping a callback future must not cancel the execution it is reading: the complete worklist
/// is still signed, and a later callback still gets its items from the same execution.
#[tokio::test(flavor = "multi_thread")]
async fn dropped_callback_future_still_completes_worklist() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let decided = fixture.seed_mixed_decided_value();
    let contributions: Vec<_> = CONTRIBUTION_SUBCOMMITTEES
        .iter()
        .map(|&subcommittee| fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, subcommittee))
        .collect();

    // Act: poll the contribution callback once, then drop it. A zero timeout polls the inner
    // future exactly once before the deadline elapses; if the callback happens to win the race the
    // scenario degrades to the sequential case.
    let stream = fixture
        .harness
        .validator_store
        .sign_sync_committee_contributions(contributions);
    let _ = tokio::time::timeout(Duration::ZERO, stream.collect::<SignContributionsResult>()).await;

    // Assert: the detached execution still signs the complete worklist.
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_batch_identity(&calls, MIXED_WORKLIST_SIZE, decided.hash());

    // A later aggregate callback joins the finished execution and gets its aggregate.
    let aggregates = vec![
        fixture
            .harness
            .create_aggregate(COMMITTEE_INDEX, AGGREGATE_VALIDATOR_IDX),
    ];
    let signed = single_batch(fixture.collect_aggregates(aggregates).await);
    assert_eq!(
        signed.len(),
        1,
        "a late aggregate callback should resolve from the cached execution"
    );

    // Still no duplicate captures after the late callback.
    tokio::time::sleep(SETTLE_DELAY).await;
    assert_eq!(
        fixture.harness.captured_calls.lock().len(),
        MIXED_WORKLIST_SIZE
    );
}

/// One validator owning an aggregate plus contributions in all four subcommittees keeps five
/// distinct signing roots in one five-entry batch.
#[tokio::test(flavor = "multi_thread")]
async fn multi_root_validator_keeps_distinct_roots() {
    // Arrange
    const EXPECTED_WORKLIST_SIZE: usize = 1 + ALL_SUBCOMMITTEES.len();
    let fixture = AggregatorCommitteeFixture::new(SINGLE_VALIDATOR_COUNT);
    let validator = fixture.validator_index(AGGREGATE_VALIDATOR_IDX);
    let contributors: Vec<_> = ALL_SUBCOMMITTEES
        .iter()
        .map(|&subcommittee| (validator, subcommittee))
        .collect();
    let decided = fixture.seed_decided_value(build_decided_data(
        &[(validator, BEACON_COMMITTEE_INDEX)],
        &contributors,
    ));
    let contributions = ALL_SUBCOMMITTEES
        .iter()
        .map(|&subcommittee| fixture.create_contribution(SOLE_VALIDATOR_IDX, subcommittee))
        .collect();

    // Act
    let results = fixture.collect_contributions(contributions).await;

    // Assert
    assert_eq!(single_batch(results).len(), ALL_SUBCOMMITTEES.len());
    assert_captured_calls_settle_at(&fixture.harness, EXPECTED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(
        distinct_roots(&calls).len(),
        EXPECTED_WORKLIST_SIZE,
        "one validator's aggregate and four contribution roots must stay distinct"
    );
    assert_batch_identity(&calls, EXPECTED_WORKLIST_SIZE, decided.hash());
    assert_eq!(
        calls_for_pubkey(&calls, fixture.pubkey(AGGREGATE_VALIDATOR_IDX)),
        EXPECTED_WORKLIST_SIZE
    );
}

// ==================== Divergent-view and normalization tests ====================

/// A requested item absent from the decided value is skipped, while the other requested
/// items succeed and the batch size counts only decided entries.
#[tokio::test(flavor = "multi_thread")]
async fn requested_item_missing_from_decided_value() {
    // Arrange: the decided value covers subcommittees 0 and 1, but the callback also asks
    // for subcommittee 2.
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let decided = fixture.seed_mixed_decided_value();
    let contributions = CONTRIBUTION_SUBCOMMITTEES
        .iter()
        .chain(std::iter::once(&MISSING_SUBCOMMITTEE_INDEX))
        .map(|&subcommittee| fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, subcommittee))
        .collect();

    // Act
    let results = fixture.collect_contributions(contributions).await;

    // Assert: only the decided subcommittees come back.
    let signed = single_batch(results);
    let returned_subcommittees: HashSet<u64> = signed
        .iter()
        .map(|item| item.message.contribution.subcommittee_index)
        .collect();
    assert_eq!(
        returned_subcommittees,
        HashSet::from(CONTRIBUTION_SUBCOMMITTEES),
        "the undecided subcommittee must be skipped, not signed"
    );

    // The batch covers the decided union only, without the missing item.
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_batch_identity(&calls, MIXED_WORKLIST_SIZE, decided.hash());
}

/// Exact duplicate decided entries collapse to one worklist entry each, so the batch size
/// reflects the deduplicated union.
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_decided_entries_normalized() {
    // Arrange: the same aggregator entry twice and the same contributor entry twice.
    const EXPECTED_DEDUPED_SIZE: usize = 2;
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let aggregator = fixture.validator_index(AGGREGATE_VALIDATOR_IDX);
    let contributor = fixture.validator_index(CONTRIBUTOR_VALIDATOR_IDX);
    let decided = fixture.seed_decided_value(build_decided_data(
        &[
            (aggregator, BEACON_COMMITTEE_INDEX),
            (aggregator, BEACON_COMMITTEE_INDEX),
        ],
        &[
            (contributor, CONTRIBUTION_SUBCOMMITTEES[0]),
            (contributor, CONTRIBUTION_SUBCOMMITTEES[0]),
        ],
    ));
    let contributions =
        vec![fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, CONTRIBUTION_SUBCOMMITTEES[0])];

    // Act
    let results = fixture.collect_contributions(contributions).await;

    // Assert
    assert_eq!(single_batch(results).len(), 1);
    assert_captured_calls_settle_at(&fixture.harness, EXPECTED_DEDUPED_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(
        distinct_roots(&calls).len(),
        EXPECTED_DEDUPED_SIZE,
        "duplicates must collapse to one entry per (validator, signing_root)"
    );
    assert_batch_identity(&calls, EXPECTED_DEDUPED_SIZE, decided.hash());
}

/// Conflicting aggregate roots for one validator sign only the first, keeping that validator
/// within the receivers' per-validator root cap while unrelated entries are unaffected.
#[tokio::test(flavor = "multi_thread")]
async fn conflicting_aggregate_roots_sign_only_the_first() {
    // Arrange: the aggregator appears twice with different committee indexes, each resolving
    // to a different attestation payload and therefore a different signing root.
    const EXPECTED_SIGNED_SIZE: usize = 2;
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let aggregator = fixture.validator_index(AGGREGATE_VALIDATOR_IDX);
    let contributor = fixture.validator_index(CONTRIBUTOR_VALIDATOR_IDX);
    let decided = fixture.seed_decided_value(build_decided_data(
        &[
            (aggregator, BEACON_COMMITTEE_INDEX),
            (aggregator, CONFLICTING_BEACON_COMMITTEE_INDEX),
        ],
        &[(contributor, CONTRIBUTION_SUBCOMMITTEES[0])],
    ));
    let contributions =
        vec![fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, CONTRIBUTION_SUBCOMMITTEES[0])];

    // Act
    let results = fixture.collect_contributions(contributions).await;

    // Assert: exactly one of the conflicting aggregates is signed, the contribution is
    // unaffected, and the batch counts both.
    assert_eq!(single_batch(results).len(), 1);
    assert_captured_calls_settle_at(&fixture.harness, EXPECTED_SIGNED_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(
        calls_for_pubkey(&calls, fixture.pubkey(AGGREGATE_VALIDATOR_IDX)),
        1,
        "a validator with conflicting decided aggregate roots must sign exactly one of them"
    );
    assert_eq!(
        calls_for_pubkey(&calls, fixture.pubkey(CONTRIBUTOR_VALIDATOR_IDX)),
        1,
        "the unrelated contribution must be unaffected by the malformed aggregate entries"
    );
    assert_eq!(
        distinct_roots(&calls).len(),
        EXPECTED_SIGNED_SIZE,
        "each signed identity contributes exactly one distinct root"
    );
    assert_batch_identity(&calls, EXPECTED_SIGNED_SIZE, decided.hash());
}

// ==================== Empty and error path tests ====================

/// A decided value naming only validators this operator has no metadata for produces an empty
/// worklist: the callbacks return empty batches and no signature is ever collected.
#[tokio::test(flavor = "multi_thread")]
async fn empty_worklist_returns_empty() {
    // Arrange: the decided entries reference foreign validator indices, while the callbacks
    // reference the local validator (required to pass committee grouping).
    let fixture = AggregatorCommitteeFixture::new(SINGLE_VALIDATOR_COUNT);
    fixture.seed_decided_value(build_decided_data(
        &[(
            ValidatorIndex(FOREIGN_AGGREGATOR_INDEX),
            BEACON_COMMITTEE_INDEX,
        )],
        &[(
            ValidatorIndex(FOREIGN_CONTRIBUTOR_INDEX),
            CONTRIBUTION_SUBCOMMITTEES[0],
        )],
    ));
    let aggregates = vec![
        fixture
            .harness
            .create_aggregate(COMMITTEE_INDEX, AGGREGATE_VALIDATOR_IDX),
    ];
    let contributions =
        vec![fixture.create_contribution(SOLE_VALIDATOR_IDX, CONTRIBUTION_SUBCOMMITTEES[0])];

    // Act
    let aggregate_results = fixture.collect_aggregates(aggregates).await;
    let contribution_results = fixture.collect_contributions(contributions).await;

    // Assert: both callbacks yield an empty batch and nothing is signed.
    assert!(
        single_batch(aggregate_results).is_empty(),
        "no locally signable decided entry means an empty aggregate batch"
    );
    assert!(
        single_batch(contribution_results).is_empty(),
        "no locally signable decided entry means an empty contribution batch"
    );
    assert_captured_calls_settle_at(&fixture.harness, 0).await;
}

/// Missing consensus data for the committee fails the execution; the trait layer swallows the
/// error into an empty batch (`run_committee_signing`) and nothing is signed.
#[tokio::test(flavor = "multi_thread")]
async fn consensus_data_missing_errors() {
    // Arrange: assignments exist for the slot, but carry no consensus data for any committee,
    // so the execution fails with `ConsensusDataNotFound`. (Seeding nothing at all would park
    // the callbacks on the assignments watch channel instead of erroring.)
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    fixture.seed_assignments_without_consensus_data();
    let aggregates = vec![
        fixture
            .harness
            .create_aggregate(COMMITTEE_INDEX, AGGREGATE_VALIDATOR_IDX),
    ];
    let contributions =
        vec![fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, CONTRIBUTION_SUBCOMMITTEES[0])];

    // Act: both callback classes hit the same cached execution error.
    let aggregate_results = fixture.collect_aggregates(aggregates).await;
    let contribution_results = fixture.collect_contributions(contributions).await;

    // Assert: `run_committee_signing` swallows the failure into one empty stream item per
    // committee, and no signature collection is ever attempted.
    assert!(
        single_batch(aggregate_results).is_empty(),
        "a failed execution should yield an empty aggregate batch"
    );
    assert!(
        single_batch(contribution_results).is_empty(),
        "a failed execution should yield an empty contribution batch"
    );
    assert_captured_calls_settle_at(&fixture.harness, 0).await;
}

/// Decided payloads whose slot does not match the duty slot are excluded from the worklist, so
/// the batch settles at the surviving count instead of stalling.
#[tokio::test(flavor = "multi_thread")]
async fn slot_mismatched_decided_payloads_are_excluded() {
    // Arrange: the standard mixed decided value, but both contribution payloads carry the
    // wrong slot; only the aggregate survives slot binding.
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let aggregator = fixture.validator_index(AGGREGATE_VALIDATOR_IDX);
    let contributor = fixture.validator_index(CONTRIBUTOR_VALIDATOR_IDX);
    let mut decided = build_decided_data(
        &[(aggregator, BEACON_COMMITTEE_INDEX)],
        &[
            (contributor, CONTRIBUTION_SUBCOMMITTEES[0]),
            (contributor, CONTRIBUTION_SUBCOMMITTEES[1]),
        ],
    );
    for contribution in decided.sync_committee_contributions.iter_mut() {
        contribution.slot = Slot::new(TEST_SLOT + 1);
    }
    fixture.seed_decided_value(decided);
    let contributions = vec![
        fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, CONTRIBUTION_SUBCOMMITTEES[0]),
        fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, CONTRIBUTION_SUBCOMMITTEES[1]),
    ];

    // Act
    let results = fixture.collect_contributions(contributions).await;

    // Assert: the requested contributions resolve to nothing (their decided payloads were
    // excluded), while the aggregate still gets signed with a batch sized to the survivors.
    assert!(
        single_batch(results).is_empty(),
        "slot-mismatched contributions should not be returned"
    );
    const EXPECTED_SURVIVOR_COUNT: usize = 1;
    assert_captured_calls_settle_at(&fixture.harness, EXPECTED_SURVIVOR_COUNT).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(
        calls[0].pubkey,
        fixture.pubkey(AGGREGATE_VALIDATOR_IDX),
        "the surviving entry should be the aggregate"
    );
    assert_eq!(
        calls[0].batch_size, EXPECTED_SURVIVOR_COUNT,
        "the batch size should count only surviving entries"
    );
}
