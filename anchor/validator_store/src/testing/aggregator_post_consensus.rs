//! Integration tests for the Boole+ `AggregatorCommittee` post-consensus execution.
//!
//! One QBFT decision carries both attestation aggregates and sync contributions. The
//! per-`(committee, slot)` execution signs the complete decided worklist regardless of which
//! Lighthouse callbacks fire. These tests pin the regression from issue #1227: a callback of one
//! class must still produce the partial signatures for the other class, so the shared committee
//! batch never stalls below its expected size.
//!
//! The two classes are read out differently. Contributions still go back through the Lighthouse
//! callback, which filters the shared outcome down to its own requested items. Aggregates do not:
//! Anchor publishes them itself from the decided value, so `resolve_decided_aggregates` and
//! `publish_decided_aggregates` are the readers, and the aggregate callback returns nothing.
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
use futures::{FutureExt, StreamExt};
use parking_lot::Mutex;
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
    SignedContributionAndProof, Slot, SyncCommitteeContribution, SyncSelectionProof,
};
use validator_store::{ContributionToSign, ValidatorStore};

use super::common::*;
use crate::{
    Error, SpecificError,
    aggregator_post_consensus::{AggregatorPostConsensusShared, ResolvedAggregates},
};

/// One committee's decided value, as the slot pipeline publishes it.
type DecidedData = AggregatorCommitteeConsensusData<MainnetEthSpec>;
/// A handle on one `(committee, slot)` post-consensus execution, as handed to the publisher.
type Execution = AggregatorPostConsensusShared<MainnetEthSpec>;

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

/// A clock position past the publisher's deadline for `TEST_SLOT` (one slot past that slot's end),
/// so deadline tests reach the timeout without sleeping two real slots.
const PAST_PUBLISH_DEADLINE_SECS: u64 = (TEST_SLOT + 3) * SLOT_DURATION_SECS;

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(2);
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Extra wait after the expected capture count is reached, to catch spurious extra captures.
const SETTLE_DELAY: Duration = Duration::from_millis(200);

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
    fn seed_decided_value(&self, data: DecidedData) -> Arc<DecidedData> {
        let data = Arc::new(data);
        self.publish_decided_value(Arc::clone(&data));
        data
    }

    /// Publishes `data` as this committee's decided value at `TEST_SLOT`, returning the executions
    /// newly registered by the publish. This is what the slot pipeline hands to the aggregate
    /// publisher, so a republish of the same slot returns nothing.
    fn publish_decided_value(&self, data: Arc<DecidedData>) -> Vec<(CommitteeId, Execution)> {
        self.harness
            .publish_decided_values(HashMap::from([(self.committee_id, data)]))
    }

    /// Seeds `AggregationAssignments` at `TEST_SLOT` without consensus data for any committee, so
    /// no execution is registered and the callbacks fail with `ConsensusDataNotFound`. Returns the
    /// (empty) set of executions the publish registered.
    fn seed_assignments_without_consensus_data(&self) -> Vec<(CommitteeId, Execution)> {
        self.harness.publish_decided_values(HashMap::new())
    }

    /// The standard mixed decided value: one aggregate for the aggregate validator plus one
    /// contribution per subcommittee in [`CONTRIBUTION_SUBCOMMITTEES`] for the contributor.
    fn mixed_decided_data(&self) -> DecidedData {
        let aggregator = self.validator_index(AGGREGATE_VALIDATOR_IDX);
        let contributor = self.validator_index(CONTRIBUTOR_VALIDATOR_IDX);
        build_decided_data(
            &[(aggregator, BEACON_COMMITTEE_INDEX)],
            &[
                (contributor, CONTRIBUTION_SUBCOMMITTEES[0]),
                (contributor, CONTRIBUTION_SUBCOMMITTEES[1]),
            ],
        )
    }

    /// Seeds the standard mixed decided value.
    fn seed_mixed_decided_value(&self) -> Arc<DecidedData> {
        self.seed_decided_value(self.mixed_decided_data())
    }

    /// Seeds the standard mixed decided value and returns the single execution it registers,
    /// for tests that also read the aggregate side through the publisher.
    fn seed_mixed_decided_value_with_execution(&self) -> (Arc<DecidedData>, Execution) {
        self.seed_decided_value_with_execution(self.mixed_decided_data())
    }

    /// Seeds `data` and returns the single execution it registers.
    fn seed_decided_value_with_execution(
        &self,
        data: DecidedData,
    ) -> (Arc<DecidedData>, Execution) {
        let data = Arc::new(data);
        let mut new_executions = self.publish_decided_value(Arc::clone(&data));
        assert_eq!(
            new_executions.len(),
            1,
            "seeding one committee should register exactly one execution"
        );
        let (committee_id, execution) = new_executions.pop().expect("execution should exist");
        assert_eq!(committee_id, self.committee_id);
        (data, execution)
    }

    /// Resolves this committee's decided aggregates the way the publisher does.
    async fn resolve_aggregates(&self, execution: Execution) -> ResolvedAggregates<MainnetEthSpec> {
        self.harness
            .validator_store
            .resolve_decided_aggregates(self.committee_id, Slot::new(TEST_SLOT), execution)
            .await
    }

    /// The aggregator index the standard mixed decided value publishes.
    fn expected_aggregator_index(&self) -> u64 {
        self.harness
            .aggregator_index(COMMITTEE_INDEX, AGGREGATE_VALIDATOR_IDX)
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

/// Asserts the publisher resolved a batch holding exactly `expected` aggregator indexes.
///
/// The non-`Batch` variants are what select the publisher's metric label, so a test that expected
/// a batch and got one of them is reported as that variant rather than as a length mismatch.
fn assert_published_aggregators(
    published: ResolvedAggregates<MainnetEthSpec>,
    expected: &[u64],
    context: &str,
) {
    match published {
        ResolvedAggregates::Batch(batch) => assert_eq!(
            batch
                .iter()
                .map(|signed| signed.message().aggregator_index())
                .collect::<Vec<_>>(),
            expected,
            "{context}"
        ),
        ResolvedAggregates::ConsensusFailed => {
            panic!("{context}: the execution failed instead of resolving a batch")
        }
        ResolvedAggregates::NoAggregates => {
            panic!("{context}: the decided worklist held no aggregates")
        }
    }
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

/// The mirror case, read through the publisher: the aggregate callback returns nothing, the
/// publisher takes the decided aggregate, and the decided contributions are signed anyway.
///
/// The aggregate callback no longer joins the execution at all, so this is the test that the
/// aggregate half of the worklist is complete: only the publisher can observe it.
#[tokio::test(flavor = "multi_thread")]
async fn publisher_takes_the_decided_aggregate_while_the_callback_returns_empty() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let (decided, execution) = fixture.seed_mixed_decided_value_with_execution();
    let aggregates = vec![
        fixture
            .harness
            .create_aggregate(COMMITTEE_INDEX, AGGREGATE_VALIDATOR_IDX),
    ];

    // Act: only the aggregate callback fires, and the publisher reads the same execution.
    let results = fixture.harness.collect_aggregates(aggregates).await;
    let published = fixture.resolve_aggregates(execution).await;

    // Assert: Lighthouse gets nothing to publish, Anchor publishes the decided aggregate itself.
    assert!(
        single_batch(results).is_empty(),
        "the aggregate callback must hand Lighthouse an empty batch at Boole+"
    );
    assert_published_aggregators(
        published,
        &[fixture.expected_aggregator_index()],
        "the publisher should return the decided aggregate",
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

/// Republishing assignments for a slot must not register a second execution.
///
/// Registration is the single writer that makes "exactly one committee message per
/// `(committee, slot)`" true. If a republish overwrote the entry it would spawn a second QBFT
/// round and a second set of detached signing tasks while the first set kept running, putting two
/// post-consensus messages on the wire for one slot, which peers reject with a gossip penalty.
#[tokio::test(flavor = "multi_thread")]
async fn republished_assignments_do_not_resubmit() {
    // Arrange: let the first publish fully submit its worklist.
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let decided = fixture.seed_mixed_decided_value();
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;

    // Act: publish the same slot again.
    fixture.seed_mixed_decided_value();

    // Assert: still exactly one submission of the worklist, not two.
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(
        distinct_roots(&calls).len(),
        MIXED_WORKLIST_SIZE,
        "a republish must not resubmit the worklist"
    );
    assert_batch_identity(&calls, MIXED_WORKLIST_SIZE, decided.hash());
}

/// The contribution callback and the publisher read the same execution: neither re-runs consensus
/// nor duplicates signatures, and each takes only its own class of decided item.
#[tokio::test(flavor = "multi_thread")]
async fn contribution_callback_and_publisher_share_one_execution() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let (decided, execution) = fixture.seed_mixed_decided_value_with_execution();
    let contributions = CONTRIBUTION_SUBCOMMITTEES
        .iter()
        .map(|&subcommittee| fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, subcommittee))
        .collect();

    // Act: contribution callback first, then the publisher joins the cached outcome.
    let contribution_results = fixture.collect_contributions(contributions).await;
    let published = fixture.resolve_aggregates(execution).await;

    // Assert: each reader took only its own class.
    assert_eq!(
        single_batch(contribution_results).len(),
        CONTRIBUTION_SUBCOMMITTEES.len(),
        "the contribution callback should return only its contributions"
    );
    assert_published_aggregators(
        published,
        &[fixture.expected_aggregator_index()],
        "the publisher should return only the decided aggregate",
    );

    // The union is signed exactly once: no duplicate captures from the second reader.
    assert_captured_calls_settle_at(&fixture.harness, MIXED_WORKLIST_SIZE).await;
    let calls = committee_calls(&fixture.harness);
    assert_eq!(distinct_roots(&calls).len(), MIXED_WORKLIST_SIZE);
    assert_batch_identity(&calls, MIXED_WORKLIST_SIZE, decided.hash());
}

/// Dropping a callback future must not cancel the execution it is reading: the complete worklist
/// is still signed, and a later reader still gets its items from the same execution.
#[tokio::test(flavor = "multi_thread")]
async fn dropped_callback_future_still_completes_worklist() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let (decided, execution) = fixture.seed_mixed_decided_value_with_execution();
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

    // A late publisher joins the finished execution and gets the decided aggregate.
    let published = fixture.resolve_aggregates(execution).await;
    assert_published_aggregators(
        published,
        &[fixture.expected_aggregator_index()],
        "a late publisher should resolve from the cached execution",
    );

    // Still no duplicate captures after the late read.
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
/// worklist: the contribution callback returns an empty batch, the publisher has nothing to
/// publish, and no signature is ever collected.
#[tokio::test(flavor = "multi_thread")]
async fn empty_worklist_returns_empty() {
    // Arrange: the decided entries reference foreign validator indices, while the callback
    // references the local validator (required to pass committee grouping).
    let fixture = AggregatorCommitteeFixture::new(SINGLE_VALIDATOR_COUNT);
    let (_, execution) = fixture.seed_decided_value_with_execution(build_decided_data(
        &[(
            ValidatorIndex(FOREIGN_AGGREGATOR_INDEX),
            BEACON_COMMITTEE_INDEX,
        )],
        &[(
            ValidatorIndex(FOREIGN_CONTRIBUTOR_INDEX),
            CONTRIBUTION_SUBCOMMITTEES[0],
        )],
    ));
    let contributions =
        vec![fixture.create_contribution(SOLE_VALIDATOR_IDX, CONTRIBUTION_SUBCOMMITTEES[0])];

    // Act
    let contribution_results = fixture.collect_contributions(contributions).await;
    let published = fixture.resolve_aggregates(execution).await;

    // Assert: nothing to return, nothing to publish, nothing signed.
    assert!(
        single_batch(contribution_results).is_empty(),
        "no locally signable decided entry means an empty contribution batch"
    );
    assert!(
        matches!(published, ResolvedAggregates::NoAggregates),
        "no locally signable decided aggregate means nothing to publish"
    );
    assert_captured_calls_settle_at(&fixture.harness, 0).await;
}

/// Missing consensus data for the committee registers no execution: the contribution callback
/// fails with `ConsensusDataNotFound`, the publisher gets no handle, and nothing is signed.
#[tokio::test(flavor = "multi_thread")]
async fn consensus_data_missing_errors() {
    // Arrange: assignments exist for the slot, but carry no consensus data for any committee,
    // so the execution fails with `ConsensusDataNotFound`. (Seeding nothing at all would park
    // the callback on the assignments watch channel instead of erroring.)
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let executions = fixture.seed_assignments_without_consensus_data();
    let contributions =
        vec![fixture.create_contribution(CONTRIBUTOR_VALIDATOR_IDX, CONTRIBUTION_SUBCOMMITTEES[0])];

    // Act
    let contribution_results = fixture.collect_contributions(contributions).await;

    // Assert: `run_committee_signing` swallows the failure into one empty stream item per
    // committee, nothing is handed to the publisher, and no signature collection is attempted.
    assert!(
        single_batch(contribution_results).is_empty(),
        "a failed execution should yield an empty contribution batch"
    );
    assert!(
        executions.is_empty(),
        "a committee with no decided value must not register a publisher handle"
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

// ==================== Publisher registration tests ====================

/// Registration is vacant-only, so a `(committee, slot)` yields its publisher handle exactly once.
///
/// The handle is what makes the publisher run, so a second handle for one slot would post the
/// same aggregates to the beacon node twice.
#[tokio::test(flavor = "multi_thread")]
async fn start_aggregator_post_consensus_returns_only_vacant_insertions() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    let decided = Arc::new(fixture.mixed_decided_data());

    // Act: publish the same slot's assignments twice.
    let first = fixture.publish_decided_value(Arc::clone(&decided));
    let second = fixture.publish_decided_value(decided);

    // Assert
    assert_eq!(
        first.len(),
        1,
        "the first publish should register this committee's execution"
    );
    assert_eq!(first[0].0, fixture.committee_id);
    assert!(
        second.is_empty(),
        "a republish must not hand out a second publisher handle"
    );
    assert_eq!(
        fixture
            .harness
            .validator_store
            .aggregator_post_consensus
            .lock()
            .len(),
        1,
        "the retention map should hold exactly one execution for the slot"
    );
}

// ==================== resolve_decided_aggregates failure tests ====================

/// A failed execution resolves to `ConsensusFailed` rather than propagating the error, which is
/// what makes the publisher count it as a consensus error and publish nothing.
#[tokio::test(flavor = "multi_thread")]
async fn resolve_decided_aggregates_reports_consensus_failure() {
    // Arrange: an execution that resolves to an error, as a lost QBFT round would.
    let fixture = AggregatorCommitteeFixture::new(SINGLE_VALIDATOR_COUNT);
    let execution: Execution = async {
        Err(Arc::new(Error::SpecificError(
            SpecificError::PostConsensusAborted,
        )))
    }
    .boxed()
    .shared();

    // Act
    let published = fixture.resolve_aggregates(execution).await;

    // Assert
    assert!(
        matches!(published, ResolvedAggregates::ConsensusFailed),
        "a failed execution should publish nothing"
    );
}

/// An execution that never decides is bounded by the two-slot deadline instead of hanging, and the
/// timeout is reported as a consensus failure.
///
/// The harness clock is moved past that deadline first, so the bound is asserted without sleeping
/// two real slots.
#[tokio::test(flavor = "multi_thread")]
async fn resolve_decided_aggregates_reports_consensus_failure_once_the_deadline_passes() {
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(SINGLE_VALIDATOR_COUNT);
    fixture
        .harness
        .slot_clock
        .set_current_time(Duration::from_secs(PAST_PUBLISH_DEADLINE_SECS));
    let execution: Execution = futures::future::pending().boxed().shared();

    // Act
    let published = tokio::time::timeout(STREAM_TIMEOUT, fixture.resolve_aggregates(execution))
        .await
        .expect("the publisher must not outlive the two-slot deadline");

    // Assert
    assert!(
        matches!(published, ResolvedAggregates::ConsensusFailed),
        "an undecided execution should publish nothing"
    );
}

// ==================== Publisher tests ====================

/// Distinct operator sets give distinct `CommitteeId`s, so one harness can hold several committees
/// with different decided values.
const PUBLISHER_OPERATOR_SETS: [[OperatorId; 4]; 3] = [
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
    [OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)],
    [OperatorId(1), OperatorId(8), OperatorId(9), OperatorId(10)],
];
/// Validator index space per publisher committee, so an aggregator index identifies its committee.
const PUBLISHER_INDEX_STRIDE: usize = 100;
/// Committee positions in [`PublisherFixture`], named by the role they play in the tests.
const FIRST_AGGREGATE_COMMITTEE: usize = 0;
const CONTRIBUTIONS_ONLY_COMMITTEE: usize = 1;
const SECOND_AGGREGATE_COMMITTEE: usize = 2;

/// Several single-validator committees in one harness, for the publisher's per-committee fan-out.
struct PublisherFixture {
    harness: ValidatorStoreTestHarness,
    committee_ids: Vec<CommitteeId>,
}

impl PublisherFixture {
    fn new() -> Self {
        let setups: Vec<_> = PUBLISHER_OPERATOR_SETS
            .iter()
            .enumerate()
            .map(|(position, operator_ids)| {
                create_committee_setup(
                    operator_ids,
                    SINGLE_VALIDATOR_COUNT,
                    (position + 1) * PUBLISHER_INDEX_STRIDE,
                )
            })
            .collect();
        let committee_ids = setups
            .iter()
            .map(|setup| setup.cluster.committee_id())
            .collect();
        Self {
            harness: ValidatorStoreTestHarness::new(setups, OUR_OPERATOR_ID),
            committee_ids,
        }
    }

    fn validator_index(&self, committee: usize) -> ValidatorIndex {
        self.harness
            .validator_metadata(committee, SOLE_VALIDATOR_IDX)
            .index
            .expect("test validator should have an index")
    }

    fn aggregator_index(&self, committee: usize) -> u64 {
        self.harness.aggregator_index(committee, SOLE_VALIDATOR_IDX)
    }

    /// A decided value holding this committee's single aggregate.
    fn aggregate_only(&self, committee: usize) -> DecidedData {
        build_decided_data(
            &[(self.validator_index(committee), BEACON_COMMITTEE_INDEX)],
            &[],
        )
    }

    /// A decided value holding one contribution and no aggregate, which the publisher must skip.
    fn contributions_only(&self, committee: usize) -> DecidedData {
        build_decided_data(
            &[],
            &[(
                self.validator_index(committee),
                CONTRIBUTION_SUBCOMMITTEES[0],
            )],
        )
    }

    /// Publishes one decided value per named committee at `TEST_SLOT`, returning the executions
    /// the slot pipeline hands to the publisher.
    fn publish(
        &self,
        decided_by_committee: Vec<(usize, DecidedData)>,
    ) -> Vec<(CommitteeId, Execution)> {
        self.harness.publish_decided_values(
            decided_by_committee
                .into_iter()
                .map(|(committee, data)| (self.committee_ids[committee], Arc::new(data)))
                .collect(),
        )
    }

    /// Runs the publisher over `executions` with a recording publish closure, returning the
    /// aggregator indexes of every batch it was handed.
    ///
    /// `outcome` decides each batch's publish result from its contents, so a test can fail one
    /// committee's publish without depending on the order committees finish in. The returned
    /// batches are sorted for the same reason.
    async fn run_publisher(
        &self,
        executions: Vec<(CommitteeId, Execution)>,
        outcome: impl Fn(&[u64]) -> Result<(), String>,
    ) -> Vec<Vec<u64>> {
        let recorded: Arc<Mutex<Vec<Vec<u64>>>> = Arc::new(Mutex::new(Vec::new()));
        self.harness
            .validator_store
            .publish_decided_aggregates(Slot::new(TEST_SLOT), executions, |signed| {
                let recorded = Arc::clone(&recorded);
                let outcome = &outcome;
                async move {
                    let aggregators: Vec<u64> = signed
                        .iter()
                        .map(|signed| signed.message().aggregator_index())
                        .collect();
                    let result = outcome(&aggregators);
                    recorded.lock().push(aggregators);
                    result
                }
            })
            .await;

        let mut recorded = recorded.lock().clone();
        recorded.sort();
        recorded
    }
}

/// Every committee with decided aggregates is published exactly once, and a committee whose
/// decided value holds only contributions never reaches the publish call at all.
///
/// Contributions stay on Lighthouse's publish path, so handing an empty batch to the beacon node
/// would be a pointless POST for the contributions-only case.
#[tokio::test(flavor = "multi_thread")]
async fn publish_decided_aggregates_publishes_each_committee_with_aggregates_once() {
    // Arrange
    let fixture = PublisherFixture::new();
    let executions = fixture.publish(vec![
        (
            FIRST_AGGREGATE_COMMITTEE,
            fixture.aggregate_only(FIRST_AGGREGATE_COMMITTEE),
        ),
        (
            CONTRIBUTIONS_ONLY_COMMITTEE,
            fixture.contributions_only(CONTRIBUTIONS_ONLY_COMMITTEE),
        ),
        (
            SECOND_AGGREGATE_COMMITTEE,
            fixture.aggregate_only(SECOND_AGGREGATE_COMMITTEE),
        ),
    ]);
    assert_eq!(
        executions.len(),
        PUBLISHER_OPERATOR_SETS.len(),
        "each committee should register one execution"
    );

    // Act
    let published = fixture.run_publisher(executions, |_| Ok(())).await;

    // Assert: one batch per committee holding aggregates, carrying that committee's aggregator.
    assert_eq!(
        published,
        vec![
            vec![fixture.aggregator_index(FIRST_AGGREGATE_COMMITTEE)],
            vec![fixture.aggregator_index(SECOND_AGGREGATE_COMMITTEE)],
        ],
        "committees with decided aggregates publish once each, and the contributions-only \
         committee never reaches the publish call"
    );
}

/// A publish failure is contained to its own committee: it neither panics nor stops the other
/// committees' batches from being published.
#[tokio::test(flavor = "multi_thread")]
async fn publish_decided_aggregates_survives_a_failing_publish() {
    // Arrange: all three committees decide an aggregate.
    let fixture = PublisherFixture::new();
    let executions = fixture.publish(
        (0..PUBLISHER_OPERATOR_SETS.len())
            .map(|committee| (committee, fixture.aggregate_only(committee)))
            .collect(),
    );
    let failing_aggregator = fixture.aggregator_index(FIRST_AGGREGATE_COMMITTEE);

    // Act: the first committee's POST fails, keyed by batch contents so the outcome does not
    // depend on which committee resolves first.
    let published = fixture
        .run_publisher(executions, |aggregators| {
            if aggregators.contains(&failing_aggregator) {
                Err("beacon node unavailable".to_string())
            } else {
                Ok(())
            }
        })
        .await;

    // Assert: every committee's batch was still attempted.
    assert_eq!(
        published,
        (0..PUBLISHER_OPERATOR_SETS.len())
            .map(|committee| vec![fixture.aggregator_index(committee)])
            .collect::<Vec<_>>(),
        "a failing publish must not withhold the other committees' batches"
    );
}

/// A committee that decided aggregates but whose roots never reached signature quorum resolves to
/// an empty `Batch`, and the publisher posts nothing.
///
/// This is a third outcome, distinct from `ConsensusFailed` (the round itself failed) and
/// `NoAggregates` (the decided value held none): consensus succeeded and there was something to
/// sign, but no signature came back. The publisher counts it under its own label, so the three
/// cases stay separable on a dashboard instead of collapsing into one.
#[tokio::test(flavor = "multi_thread")]
async fn publish_decided_aggregates_skips_a_committee_whose_roots_never_reach_quorum() {
    // Arrange: signature collection fails before the decided value is published, so the
    // committee's only aggregate root resolves to an error.
    let fixture = PublisherFixture::new();
    fixture.harness.fail_signature_collection();
    let executions = fixture.publish(vec![(
        FIRST_AGGREGATE_COMMITTEE,
        fixture.aggregate_only(FIRST_AGGREGATE_COMMITTEE),
    )]);
    let (committee_id, execution) = executions
        .first()
        .cloned()
        .expect("the committee should register one execution");

    // Act
    let resolved = fixture
        .harness
        .validator_store
        .resolve_decided_aggregates(committee_id, Slot::new(TEST_SLOT), execution)
        .await;
    let published = fixture.run_publisher(executions, |_| Ok(())).await;

    // Assert: a batch was resolved, it is empty, and nothing was handed to publish.
    assert!(
        matches!(&resolved, ResolvedAggregates::Batch(batch) if batch.is_empty()),
        "a decided aggregate whose signature never arrives should resolve to an empty batch, \
         not to a consensus failure or an absent worklist"
    );
    assert!(
        published.is_empty(),
        "the publisher must not POST an empty batch"
    );
}
