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
//! Anchor publishes them itself from the decided value, one publish call per root, so
//! `publish_decided_aggregates` is the reader and the aggregate callback returns nothing.
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
    AttestationBase, AttestationData, AttestationElectra, Checkpoint, Epoch, ForkName, Hash256,
    MainnetEthSpec, SignedContributionAndProof, Slot, SyncCommitteeContribution,
    SyncSelectionProof,
};
use validator_store::{ContributionToSign, ValidatorStore};

use super::common::*;
use crate::{
    Error, SpecificError, aggregator_post_consensus::AggregatorPostConsensusShared, metrics,
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

    /// Runs the publisher over this committee's execution with an always-succeeding recording
    /// closure, returning one `(fork, aggregator index)` entry per publish call.
    async fn run_publisher(&self, execution: Execution) -> Vec<RecordedPublish> {
        run_recording_publisher(&self.harness, vec![(self.committee_id, execution)], |_| {
            Ok(())
        })
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

/// The `AttestationData` shared by the aggregate payload builders, at `TEST_SLOT`.
fn test_attestation_data(index: u64) -> AttestationData {
    AttestationData {
        slot: Slot::new(TEST_SLOT),
        index,
        beacon_block_root: Hash256::zero(),
        source: Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::zero(),
        },
        target: Checkpoint {
            epoch: Epoch::new(0),
            root: Hash256::zero(),
        },
    }
}

/// An aggregate attestation payload at `TEST_SLOT`, in the pre-Electra shape. Distinct
/// `committee_index` values produce distinct signing roots.
fn test_aggregate_attestation(committee_index: u64) -> AttestationBase<MainnetEthSpec> {
    AttestationBase {
        aggregation_bits: ssz_types::BitList::with_capacity(128).expect("bitlist should be valid"),
        data: test_attestation_data(committee_index),
        signature: AggregateSignature::infinity(),
    }
}

/// An aggregate attestation payload at `TEST_SLOT`, in the Electra+ shape: the committee moves
/// from `data.index` to `committee_bits`. Distinct `committee_index` values produce distinct
/// signing roots.
fn test_aggregate_attestation_electra(committee_index: u64) -> AttestationElectra<MainnetEthSpec> {
    let mut committee_bits = ssz_types::BitVector::new();
    committee_bits
        .set(committee_index as usize, true)
        .expect("committee bit should be in range");
    AttestationElectra {
        aggregation_bits: ssz_types::BitList::with_capacity(128).expect("bitlist should be valid"),
        data: test_attestation_data(0),
        signature: AggregateSignature::infinity(),
        committee_bits,
    }
}

/// SSZ payload bytes for one decided aggregate, in the shape `fork` decodes.
fn aggregate_payload_bytes(fork: ForkName, committee_index: u64) -> Vec<u8> {
    if fork >= ForkName::Electra {
        test_aggregate_attestation_electra(committee_index).as_ssz_bytes()
    } else {
        test_aggregate_attestation(committee_index).as_ssz_bytes()
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

/// Fork the standard decided values are built at; [`build_decided_data`] uses it.
const DEFAULT_DECIDED_FORK: ForkName = ForkName::Deneb;

/// Builds an `AggregatorCommitteeConsensusData` at [`DEFAULT_DECIDED_FORK`] from decided entries.
fn build_decided_data(
    aggregators: &[(ValidatorIndex, u64)],
    contributors: &[(ValidatorIndex, u64)],
) -> AggregatorCommitteeConsensusData<MainnetEthSpec> {
    build_decided_data_at_fork(DEFAULT_DECIDED_FORK, aggregators, contributors)
}

/// Builds an `AggregatorCommitteeConsensusData` decided at `fork` from decided entries.
///
/// `aggregators` and `contributors` are `(validator_index, committee_index)` pairs; the
/// attestation and contribution payload lists are derived from them (one payload per unique
/// index), with each attestation payload in the shape `fork` decodes.
fn build_decided_data_at_fork(
    fork: ForkName,
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
            VariableList::new(aggregate_payload_bytes(fork, committee_index))
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
        version: DataVersion::from(fork),
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

// ==================== Recording publisher ====================

/// Fork and aggregator index of one publish call made by the recording closure.
type RecordedPublish = (ForkName, u64);

/// Runs the publisher over `executions` with a recording publish closure, returning one
/// `(fork, aggregator index)` entry per publish call.
///
/// The recorder captures the `ForkName` each call receives, so tests can pin that it matches the
/// decided value's `DataVersion`. `outcome` decides each call's publish result from its
/// aggregator index, so a test can fail one aggregate's publish without depending on the order
/// roots finish in. The returned calls are sorted by aggregator index for the same reason.
async fn run_recording_publisher(
    harness: &ValidatorStoreTestHarness,
    executions: Vec<(CommitteeId, Execution)>,
    outcome: impl Fn(u64) -> Result<(), String>,
) -> Vec<RecordedPublish> {
    let recorded: Arc<Mutex<Vec<RecordedPublish>>> = Arc::new(Mutex::new(Vec::new()));
    harness
        .validator_store
        .publish_decided_aggregates(Slot::new(TEST_SLOT), executions, |fork_name, signed| {
            let recorded = Arc::clone(&recorded);
            let outcome = &outcome;
            async move {
                let aggregator = signed.message().aggregator_index();
                let result = outcome(aggregator);
                recorded.lock().push((fork_name, aggregator));
                result
            }
        })
        .await;

    let mut recorded = recorded.lock().clone();
    recorded.sort_by_key(|&(_, aggregator)| aggregator);
    recorded
}

/// Serializes the tests that mutate and assert deltas on the `consensus_error`, `no_aggregates`,
/// and `no_signatures` publish outcome labels, so one test's increments cannot land inside
/// another's before/after window. `success` and `http_error` have no delta assertions, so tests
/// touching only those labels stay unserialized.
static PUBLISH_METRIC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Current value of one `AGGREGATOR_COMMITTEE_PUBLISH_TOTAL` label.
///
/// The counters are process-global: a test asserting a delta must hold [`PUBLISH_METRIC_LOCK`],
/// together with every test that increments the same label.
fn publish_result_count(label: &str) -> u64 {
    metrics::AGGREGATOR_COMMITTEE_PUBLISH_TOTAL
        .as_ref()
        .expect("the publish outcome metric should be created")
        .with_label_values(&[label])
        .get()
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

/// Asserts the recording publisher made exactly one publish call per expected aggregator index,
/// all at [`DEFAULT_DECIDED_FORK`].
fn assert_published_aggregators(published: &[RecordedPublish], expected: &[u64], context: &str) {
    let expected: Vec<RecordedPublish> = expected
        .iter()
        .map(|&aggregator| (DEFAULT_DECIDED_FORK, aggregator))
        .collect();
    assert_eq!(published, expected.as_slice(), "{context}");
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
    let published = fixture.run_publisher(execution).await;

    // Assert: Lighthouse gets nothing to publish, Anchor publishes the decided aggregate itself.
    assert!(
        single_batch(results).is_empty(),
        "the aggregate callback must hand Lighthouse an empty batch at Boole+"
    );
    assert_published_aggregators(
        &published,
        &[fixture.expected_aggregator_index()],
        "the publisher should publish the decided aggregate",
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
    let published = fixture.run_publisher(execution).await;

    // Assert: each reader took only its own class.
    assert_eq!(
        single_batch(contribution_results).len(),
        CONTRIBUTION_SUBCOMMITTEES.len(),
        "the contribution callback should return only its contributions"
    );
    assert_published_aggregators(
        &published,
        &[fixture.expected_aggregator_index()],
        "the publisher should publish only the decided aggregate",
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
    let published = fixture.run_publisher(execution).await;
    assert_published_aggregators(
        &published,
        &[fixture.expected_aggregator_index()],
        "a late publisher should publish from the cached execution",
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
/// publish and counts the committee under `no_aggregates`, and no signature is ever collected.
#[tokio::test(flavor = "multi_thread")]
async fn empty_worklist_returns_empty() {
    let _metric_guard = PUBLISH_METRIC_LOCK.lock().await;
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
    let no_aggregates_before = publish_result_count(metrics::NO_AGGREGATES);

    // Act
    let contribution_results = fixture.collect_contributions(contributions).await;
    let published = fixture.run_publisher(execution).await;

    // Assert: nothing to return, nothing to publish, nothing signed.
    assert!(
        single_batch(contribution_results).is_empty(),
        "no locally signable decided entry means an empty contribution batch"
    );
    assert!(
        published.is_empty(),
        "no locally signable decided aggregate means no publish call"
    );
    assert_eq!(
        publish_result_count(metrics::NO_AGGREGATES),
        no_aggregates_before + 1,
        "the publisher should count the aggregate-free committee under `no_aggregates`"
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

// ==================== Publisher consensus-failure tests ====================

/// A failed execution publishes nothing and is counted as a consensus error rather than
/// propagating the error.
#[tokio::test(flavor = "multi_thread")]
async fn publisher_counts_a_failed_execution_as_a_consensus_error() {
    let _metric_guard = PUBLISH_METRIC_LOCK.lock().await;
    // Arrange: an execution that resolves to an error, as a lost QBFT round would.
    let fixture = AggregatorCommitteeFixture::new(SINGLE_VALIDATOR_COUNT);
    let execution: Execution = async {
        Err(Arc::new(Error::SpecificError(
            SpecificError::PostConsensusAborted,
        )))
    }
    .boxed()
    .shared();
    let consensus_error_before = publish_result_count(metrics::CONSENSUS_ERROR);

    // Act
    let published = fixture.run_publisher(execution).await;

    // Assert
    assert!(
        published.is_empty(),
        "a failed execution should publish nothing"
    );
    assert_eq!(
        publish_result_count(metrics::CONSENSUS_ERROR),
        consensus_error_before + 1,
        "the publisher should count the failed committee under `consensus_error`"
    );
}

/// An execution that never decides is bounded by the two-slot deadline instead of hanging, and
/// the timeout is counted as a consensus error.
///
/// The harness clock is moved past that deadline first, so the bound is asserted without sleeping
/// two real slots.
#[tokio::test(flavor = "multi_thread")]
async fn publisher_counts_a_consensus_error_once_the_deadline_passes() {
    let _metric_guard = PUBLISH_METRIC_LOCK.lock().await;
    // Arrange
    let fixture = AggregatorCommitteeFixture::new(SINGLE_VALIDATOR_COUNT);
    fixture
        .harness
        .slot_clock
        .set_current_time(Duration::from_secs(PAST_PUBLISH_DEADLINE_SECS));
    let execution: Execution = futures::future::pending().boxed().shared();
    let consensus_error_before = publish_result_count(metrics::CONSENSUS_ERROR);

    // Act
    let published = tokio::time::timeout(STREAM_TIMEOUT, fixture.run_publisher(execution))
        .await
        .expect("the publisher must not outlive the two-slot deadline");

    // Assert
    assert!(
        published.is_empty(),
        "an undecided execution should publish nothing"
    );
    assert_eq!(
        publish_result_count(metrics::CONSENSUS_ERROR),
        consensus_error_before + 1,
        "the publisher should count the timed-out committee under `consensus_error`"
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
        self.aggregate_only_at_fork(committee, DEFAULT_DECIDED_FORK)
    }

    /// [`Self::aggregate_only`] decided at `fork`, with the payload in that fork's shape.
    fn aggregate_only_at_fork(&self, committee: usize, fork: ForkName) -> DecidedData {
        build_decided_data_at_fork(
            fork,
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

    /// [`run_recording_publisher`] over this fixture's harness.
    async fn run_publisher(
        &self,
        executions: Vec<(CommitteeId, Execution)>,
        outcome: impl Fn(u64) -> Result<(), String>,
    ) -> Vec<RecordedPublish> {
        run_recording_publisher(&self.harness, executions, outcome).await
    }
}

/// Every decided aggregate is published exactly once, and a committee whose decided value holds
/// only contributions never reaches the publish call at all.
///
/// Contributions stay on Lighthouse's publish path, so a publish call for the contributions-only
/// committee would be a pointless POST.
#[tokio::test(flavor = "multi_thread")]
async fn publish_decided_aggregates_publishes_each_committee_with_aggregates_once() {
    let _metric_guard = PUBLISH_METRIC_LOCK.lock().await;
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

    // Assert: one publish call per decided aggregate, carrying its committee's aggregator and
    // the decided value's fork.
    assert_eq!(
        published,
        vec![
            (
                DEFAULT_DECIDED_FORK,
                fixture.aggregator_index(FIRST_AGGREGATE_COMMITTEE),
            ),
            (
                DEFAULT_DECIDED_FORK,
                fixture.aggregator_index(SECOND_AGGREGATE_COMMITTEE),
            ),
        ],
        "committees with decided aggregates publish once each, and the contributions-only \
         committee never reaches the publish call"
    );
}

/// A publish failure is contained to its own aggregate: it neither panics nor stops the other
/// committees' aggregates from being published.
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

    // Act: the first committee's POST fails, keyed by aggregator index so the outcome does not
    // depend on which committee resolves first.
    let published = fixture
        .run_publisher(executions, |aggregator| {
            if aggregator == failing_aggregator {
                Err("beacon node unavailable".to_string())
            } else {
                Ok(())
            }
        })
        .await;

    // Assert: every committee's aggregate was still attempted.
    assert_eq!(
        published,
        (0..PUBLISHER_OPERATOR_SETS.len())
            .map(|committee| (DEFAULT_DECIDED_FORK, fixture.aggregator_index(committee)))
            .collect::<Vec<_>>(),
        "a failing publish must not withhold the other committees' aggregates"
    );
}

/// A committee that decided an aggregate whose root never reached signature quorum publishes
/// nothing, and the root is counted under `no_signatures`.
///
/// This is a third outcome, distinct from `consensus_error` (the round itself failed) and
/// `no_aggregates` (the decided value held none): consensus succeeded and there was something to
/// sign, but no signature came back. The publisher counts each such root under its own label, so
/// the three cases stay separable on a dashboard instead of collapsing into one.
#[tokio::test(flavor = "multi_thread")]
async fn publish_decided_aggregates_skips_a_committee_whose_roots_never_reach_quorum() {
    let _metric_guard = PUBLISH_METRIC_LOCK.lock().await;
    // Arrange: signature collection fails before the decided value is published, so the
    // committee's only aggregate root resolves to an error.
    let fixture = PublisherFixture::new();
    fixture.harness.fail_signature_collection();
    let executions = fixture.publish(vec![(
        FIRST_AGGREGATE_COMMITTEE,
        fixture.aggregate_only(FIRST_AGGREGATE_COMMITTEE),
    )]);
    let no_signatures_before = publish_result_count(metrics::NO_SIGNATURES);

    // Act
    let published = fixture.run_publisher(executions, |_| Ok(())).await;

    // Assert: the publish closure was never invoked, and the root was counted under its label.
    assert!(
        published.is_empty(),
        "the publisher must not invoke the publish closure for a root without quorum"
    );
    assert_eq!(
        publish_result_count(metrics::NO_SIGNATURES),
        no_signatures_before + 1,
        "each decided root without quorum should count once under `no_signatures`; this \
         committee decided exactly one root"
    );
}

/// Two decided aggregate roots in one committee where exactly one misses signature quorum: the
/// surviving root is still published, and only the missing root is counted under
/// `no_signatures`.
///
/// This per-root independence is the point of the per-root publisher: under the batch shape a
/// committee published all-or-nothing, so one quorum miss could withhold a signed sibling.
#[tokio::test(flavor = "multi_thread")]
async fn publish_decided_aggregates_publishes_the_surviving_root_when_a_sibling_misses_quorum() {
    let _metric_guard = PUBLISH_METRIC_LOCK.lock().await;
    // Arrange: both validators aggregate distinct roots, and signature collection fails only for
    // the second validator. The fail mode is set before seeding, since the detached worklist
    // tasks start collecting at publish.
    const SURVIVING_VALIDATOR_IDX: usize = 0;
    const FAILING_VALIDATOR_IDX: usize = 1;
    let fixture = AggregatorCommitteeFixture::new(MIXED_COMMITTEE_VALIDATOR_COUNT);
    fixture
        .harness
        .fail_signature_collection_for(fixture.pubkey(FAILING_VALIDATOR_IDX));
    let (_, execution) = fixture.seed_decided_value_with_execution(build_decided_data(
        &[
            (
                fixture.validator_index(SURVIVING_VALIDATOR_IDX),
                BEACON_COMMITTEE_INDEX,
            ),
            (
                fixture.validator_index(FAILING_VALIDATOR_IDX),
                CONFLICTING_BEACON_COMMITTEE_INDEX,
            ),
        ],
        &[],
    ));
    let no_signatures_before = publish_result_count(metrics::NO_SIGNATURES);

    // Act
    let published = fixture.run_publisher(execution).await;

    // Assert: exactly the surviving root was published, exactly the missing one was counted.
    assert_eq!(
        published,
        vec![(
            DEFAULT_DECIDED_FORK,
            fixture
                .harness
                .aggregator_index(COMMITTEE_INDEX, SURVIVING_VALIDATOR_IDX),
        )],
        "a sibling's quorum miss must not withhold the surviving root's publish call"
    );
    assert_eq!(
        publish_result_count(metrics::NO_SIGNATURES),
        no_signatures_before + 1,
        "only the root that missed quorum should count under `no_signatures`"
    );
}

/// The fork handed to the publish closure is the decided value's `DataVersion` fork, not any
/// local default: an Electra-decided value reaches the closure as `ForkName::Electra`.
///
/// The closure derives its publish endpoint and fork header from this argument, so binding it to
/// the decided version is what keeps the endpoint from diverging from the payload variant.
#[tokio::test(flavor = "multi_thread")]
async fn publish_decided_aggregates_hands_the_decided_versions_fork_to_the_closure() {
    // Arrange: one committee decides an Electra-versioned value with an Electra-shaped payload.
    let fixture = PublisherFixture::new();
    let executions = fixture.publish(vec![(
        FIRST_AGGREGATE_COMMITTEE,
        fixture.aggregate_only_at_fork(FIRST_AGGREGATE_COMMITTEE, ForkName::Electra),
    )]);

    // Act
    let published = fixture.run_publisher(executions, |_| Ok(())).await;

    // Assert
    assert_eq!(
        published,
        vec![(
            ForkName::Electra,
            fixture.aggregator_index(FIRST_AGGREGATE_COMMITTEE),
        )],
        "the publish closure must receive the fork named by the decided value's DataVersion"
    );
}
