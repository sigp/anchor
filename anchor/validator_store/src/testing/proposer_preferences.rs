//! Integration tests for the ProposerPreferences path in `sign_proposer_preferences()`.
//!
//! These tests pin the two decisions that are unique to this duty and share no coverage with any
//! other signing path:
//!  - the signing domain is keyed by the *proposal* slot's epoch (`Domain::ProposerPreferences`),
//!  - the partial-signature collection is scheduled at the future `proposal_slot` itself (SIP-94
//!    §5/§7), not `slot_clock.now()` and not the proposal epoch's start slot. Peers accept the
//!    future slot via the ProposerPreferences role's earliness allowance and validate proposer
//!    assignment against it (#1062), and keying the collector to `proposal_slot` keeps it alive
//!    until `proposal_slot + 1` passes.
//!
//! Scope of the slot assertions: these tests assert `call.metadata.slot`, the value captured by
//! the mock at the `sign_and_collect` trait boundary. The real `create_message` that builds the
//! on-wire `PartialSignatureMessages` never runs here, so the wire-slot field is not observed
//! directly. `create_message` copies `metadata.slot` verbatim into `PartialSignatureMessages.slot`
//! (see `signature_collector::SignatureCollectorManager::create_message`), and that verbatim copy
//! is covered by signature_collector's own tests; asserting `metadata.slot` therefore pins the
//! input to that copy.
use std::sync::LazyLock;

use signature_collector::{CollectionError, SignatureRequester};
use ssv_types::{OperatorId, msgid::Role, partial_sig::PartialSignatureKind};
use types::{
    Address, ChainSpec, Domain, Epoch, EthSpec, Hash256, MainnetEthSpec, ProposerPreferences,
    SignedRoot, Slot,
};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{Error, SpecificError};

/// Non-zero so a correctly echoed beacon index is distinguishable from an accidental default 0.
const STARTING_VALIDATOR_INDEX: usize = 5;
/// Distinctive fee recipient / gas limit so an echoed message is distinguishable from a default.
const TEST_GAS_LIMIT: u64 = 30_000_000;
/// Number of epochs a lookahead proposal slot sits ahead of the send slot. Two epochs places the
/// proposal slot at a value distinct from both `TEST_SLOT` and the proposal epoch's start slot, so
/// the envelope-slot test can rule out both alternatives.
const LOOKAHEAD_EPOCHS: u64 = 2;

/// Serializes the two metric tests against each other. Both read the same labels of the global
/// prometheus `PROPOSER_PREFERENCES_RECONSTRUCTION_FAILURES` counter, so concurrent execution
/// would make their cross-label delta assertions racy. A tokio mutex rather than std because the
/// guard is held across awaits on a multi-thread runtime.
static METRIC_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn test_operator_ids() -> [OperatorId; 4] {
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)]
}

/// Builds a `ProposerPreferences` fixture. The store signs whatever it is handed, so a fixed
/// fixture is sufficient; `proposal_slot` is a parameter because it keys the signing domain and
/// the envelope-slot behavior the tests assert on.
fn create_proposer_preferences(validator_index: u64, proposal_slot: Slot) -> ProposerPreferences {
    ProposerPreferences {
        dependent_root: Hash256::from([7u8; 32]),
        proposal_slot,
        validator_index,
        fee_recipient: Address::repeat_byte(0xab),
        target_gas_limit: TEST_GAS_LIMIT,
    }
}

/// Independently recomputes the expected signing root for a `ProposerPreferences` under the
/// `Domain::ProposerPreferences` domain keyed at an explicit `epoch`, using the harness's own
/// `spec` and `genesis_validators_root` (rather than hardcoding a spec / zero root) so the
/// recompute exactly tracks the store's fork selection.
///
/// `epoch` is a parameter, not derived from `proposal_slot`, so a caller can recompute the root
/// under both the proposal epoch (the correct key) and the send epoch (the regressed key) and
/// contrast them. On a spec with a fork boundary between those two epochs the domains differ, which
/// is what makes the "keyed on the proposal epoch, not the send epoch" assertion falsifiable.
fn expected_signing_root(
    preferences: &ProposerPreferences,
    spec: &ChainSpec,
    genesis_validators_root: Hash256,
    epoch: Epoch,
) -> Hash256 {
    let domain = spec.get_domain(
        epoch,
        Domain::ProposerPreferences,
        &spec.fork_at_epoch(epoch),
        genesis_validators_root,
    );
    preferences.signing_root(domain)
}

// ==================== Success / signing-root tests ====================

/// `sign_proposer_preferences` collects a single-validator signature and echoes the input back in
/// the resulting message, committing to the `ProposerPreferences` under the `ProposerPreferences`
/// domain keyed by the proposal slot's epoch.
///
/// Uses a lookahead `proposal_slot` (LOOKAHEAD_EPOCHS ahead of the send slot) so the recomputed
/// domain epoch is `epoch(proposal_slot)` for a *future* slot, not the send slot's epoch.
/// Critically, the harness runs on a spec with Gloas activated exactly at `LOOKAHEAD_EPOCHS`, so a
/// fork boundary sits strictly between the send epoch (0, genesis fork version) and the proposal
/// epoch (`LOOKAHEAD_EPOCHS`, Gloas fork version). Those two fork versions produce byte-distinct
/// signing domains, so the test can both (a) assert the observed root matches a recompute keyed at
/// the proposal epoch and (b) assert it does NOT match a recompute keyed at the send epoch.
/// Assertion (b) is the falsifiability guard: a regression to keying the domain on the send epoch
/// would flip it, whereas under `ChainSpec::mainnet()` (send and proposal epochs both pre-Altair)
/// both epochs share the genesis fork version and the guard could not distinguish them.
///
/// It also asserts `call.metadata.slot` (the value captured at the collector boundary, which
/// `create_message` copies verbatim into `PartialSignatureMessages.slot`) equals
/// `preferences.proposal_slot`, the SIP-94 §5/§7 wire-slot invariant.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_preferences_reconstruction_threshold() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    // Gloas at epoch LOOKAHEAD_EPOCHS puts a fork boundary strictly inside the lookahead window, so
    // the proposal epoch (Gloas) and the send epoch (genesis) resolve to different signing domains.
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            spec: gloas_at_epoch_spec(Epoch::new(LOOKAHEAD_EPOCHS)),
            ..Default::default()
        },
    );
    // A future proposal slot so `epoch(proposal_slot)` differs from the send slot's epoch; with the
    // Gloas boundary at LOOKAHEAD_EPOCHS, `epoch(proposal_slot) == LOOKAHEAD_EPOCHS` is the Gloas
    // epoch while the send epoch (0) stays on the genesis fork version.
    let future_proposal_slot =
        Slot::new(TEST_SLOT + MainnetEthSpec::slots_per_epoch() * LOOKAHEAD_EPOCHS);
    let preferences =
        create_proposer_preferences(STARTING_VALIDATOR_INDEX as u64, future_proposal_slot);

    // Act
    let result = harness
        .validator_store
        .sign_proposer_preferences(pubkey, preferences.clone())
        .await;

    // Assert
    let signed = result.expect("proposer preferences signing should succeed");
    assert_eq!(
        signed.message, preferences,
        "signed message should echo the input preferences unchanged"
    );

    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        1,
        "expected exactly one sign_and_collect call"
    );
    let call = &captured[0];
    match &call.requester {
        SignatureRequester::SingleValidator {
            pubkey: requester_pubkey,
        } => assert_eq!(
            *requester_pubkey, pubkey,
            "collection should be requested for the signing validator"
        ),
        other => panic!("expected SignatureRequester::SingleValidator, got: {other:?}"),
    }
    assert_eq!(
        call.metadata.kind,
        PartialSignatureKind::ProposerPreferences,
        "partial signature messages should be tagged with the ProposerPreferences kind"
    );
    assert_eq!(
        call.metadata.role,
        Role::ProposerPreferences,
        "the network message should be routed under the ProposerPreferences role"
    );
    // `call.metadata.slot` (captured at the collector boundary) must equal `proposal_slot`: SIP-94
    // §5/§7 pins the wire slot to the slot being proposed, and #1062 validates proposer assignment
    // against it. `create_message` copies this value verbatim into `PartialSignatureMessages.slot`.
    assert_eq!(
        call.metadata.slot, preferences.proposal_slot,
        "the collected partial-signature slot should equal the proposal slot"
    );

    // Recompute the root independently to lock the ProposerPreferences-specific signing decisions
    // that no other test covers: the `Domain::ProposerPreferences` choice, the epoch keyed off
    // `proposal_slot`, and the signed object being the `ProposerPreferences` itself. Recompute
    // under both epochs and assert the observed root matches the proposal epoch but NOT the send
    // epoch. The inequality is meaningful only because the Gloas boundary at LOOKAHEAD_EPOCHS makes
    // the two epochs' domains differ; a regression to send-epoch keying would satisfy the first
    // assert but fail on flipping to match the second recompute.
    let slots_per_epoch = MainnetEthSpec::slots_per_epoch();
    let proposal_epoch = preferences.proposal_slot.epoch(slots_per_epoch);
    let send_epoch = Slot::new(TEST_SLOT).epoch(slots_per_epoch);
    let expected_root = expected_signing_root(
        &preferences,
        &harness.spec,
        harness.genesis_validators_root,
        proposal_epoch,
    );
    assert_eq!(
        call.signing_root, expected_root,
        "signing root should commit to the proposer preferences under the ProposerPreferences \
         domain keyed by the proposal slot's epoch"
    );
    let send_epoch_root = expected_signing_root(
        &preferences,
        &harness.spec,
        harness.genesis_validators_root,
        send_epoch,
    );
    assert_ne!(
        call.signing_root, send_epoch_root,
        "signing root must NOT be keyed by the send slot's epoch: the proposal epoch (Gloas) and \
         the send epoch (genesis) yield different domains, so send-epoch keying is observable here"
    );
}

/// The partial-signature collection is scheduled at the future `proposal_slot` carried in the
/// body, NOT at the current send slot (`slot_clock.now()`) and NOT at the proposal epoch's start
/// slot. SIP-94 §5/§7 pins the on-wire `PartialSignatureMessages.slot` to `proposal_slot` itself
/// (one runner per proposal slot, matching ssv-spec's `msg.Slot == duty.DutySlot()`). Peers accept
/// the future slot via the ProposerPreferences role's earliness allowance and validate proposer
/// assignment against it (#1062); keying the collector to `proposal_slot` also keeps it alive until
/// `proposal_slot + 1` passes, so lookahead emissions no longer race a 1-slot arrival window. This
/// is the opposite rationale of the earlier send-slot design.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_preferences_envelope_slot_is_proposal_slot() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);

    // A proposal LOOKAHEAD_EPOCHS in the future: distinct from the send slot AND from that future
    // epoch's start slot, so the assertions below can rule out both alternatives.
    let future_proposal_slot =
        Slot::new(TEST_SLOT + MainnetEthSpec::slots_per_epoch() * LOOKAHEAD_EPOCHS);
    let preferences =
        create_proposer_preferences(STARTING_VALIDATOR_INDEX as u64, future_proposal_slot);

    // Act
    let result = harness
        .validator_store
        .sign_proposer_preferences(pubkey, preferences.clone())
        .await;

    // Assert
    result.expect("proposer preferences signing should succeed for a future proposal slot");

    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        1,
        "expected exactly one sign_and_collect call"
    );
    let call = &captured[0];

    // `call.metadata.slot` (captured at the collector boundary) must be the future `proposal_slot`
    // (SIP-94 §5/§7). `create_message` copies it verbatim into `PartialSignatureMessages.slot`, so
    // the on-wire slot equals the slot the preference targets: this is what peers validate proposer
    // assignment against (#1062) and what keeps the collector alive until `proposal_slot + 1`.
    assert_eq!(
        call.metadata.slot, preferences.proposal_slot,
        "collection should be scheduled at the future proposal slot"
    );
    // Explicitly rule out the two rejected alternatives: the current send slot and the proposal
    // epoch's start slot. `LOOKAHEAD_EPOCHS >= 1` guarantees `proposal_slot` differs from both.
    assert_ne!(
        call.metadata.slot,
        Slot::new(TEST_SLOT),
        "the envelope slot must NOT be the current send slot (slot_clock.now())"
    );
    let proposal_epoch_start = preferences
        .proposal_slot
        .epoch(MainnetEthSpec::slots_per_epoch())
        .start_slot(MainnetEthSpec::slots_per_epoch());
    assert_ne!(
        call.metadata.slot, proposal_epoch_start,
        "the envelope slot must NOT be the proposal epoch's start slot"
    );
}

// ==================== Failure classification / metrics tests ====================

/// A collection failure that classifies as `NoSignature` propagates the real
/// `SignatureCollectionFailed` error (not `Unsupported`) and increments the
/// `insufficient_partial_signatures` reconstruction-failure metric exactly once.
///
/// Log-field assertion path: the codebase has no tracing/log-capture utility (no `tracing-test`
/// dependency, no `logs_contain`/subscriber-capture helper), so we deliberately do NOT build
/// fragile tracing infrastructure. The warn-log fields (validator index, proposal slot,
/// `target_gas_limit`, `dependent_root`, signing root) and the `insufficient_partial_signatures`
/// metric increment are emitted by the *same* `NoSignature` match arm in
/// `report_proposer_preferences_collection_failure`, so the metric increment proves that warn
/// branch executed; the log-field assertion is covered indirectly.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_preferences_insufficient_partial_signatures_warns_and_metrics() {
    // The global prometheus registry makes cross-label delta assertions racy between the two
    // metric tests, so they serialize against each other.
    let _guard = METRIC_TEST_LOCK.lock().await;

    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            // QueueClosedError classifies as the NoSignature / insufficient-partial-signatures
            // class.
            collector_failure: Some(CollectionError::QueueClosedError),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let preferences =
        create_proposer_preferences(STARTING_VALIDATOR_INDEX as u64, Slot::new(TEST_SLOT));

    // The metric lives in the global prometheus registry shared by every test in the process, so
    // we assert on deltas. The infra metric test also touches these labels, so the deltas are only
    // reliable because both tests hold `METRIC_TEST_LOCK`; future failure tests must join that
    // serialization or use distinct labels.
    let metric = crate::metrics::PROPOSER_PREFERENCES_RECONSTRUCTION_FAILURES
        .as_ref()
        .expect("metric should be created");
    let insufficient_counter = metric.with_label_values(&[
        crate::metrics::PROPOSER_PREFERENCES_FAILURE_INSUFFICIENT_PARTIAL_SIGNATURES,
    ]);
    let infra_counter =
        metric.with_label_values(&[crate::metrics::PROPOSER_PREFERENCES_FAILURE_INFRA]);
    let insufficient_before = insufficient_counter.get();
    let infra_before = infra_counter.get();

    // Act
    let result = harness
        .validator_store
        .sign_proposer_preferences(pubkey, preferences)
        .await;

    // Assert
    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::SignatureCollectionFailed(CollectionError::QueueClosedError)
            ))
        ),
        "expected QueueClosedError surfaced as SignatureCollectionFailed (the real error, not \
         Unsupported), got: {result:?}"
    );
    assert_eq!(
        insufficient_counter.get() - insufficient_before,
        1,
        "QueueClosedError should increment the insufficient_partial_signatures reconstruction-failure \
         metric once"
    );
    // Pins the classification boundary: QueueClosedError is a NoSignature-class failure and must
    // NOT be attributed to the infra bucket. A regression that reclassified it as infra would flip
    // this zero delta.
    assert_eq!(
        infra_counter.get() - infra_before,
        0,
        "QueueClosedError must not leak into the infra reconstruction-failure metric"
    );
}

/// An infrastructure collection failure (`EmptySignature`) propagates the real
/// `SignatureCollectionFailed` error and increments the `infra` reconstruction-failure metric,
/// leaving the `insufficient_partial_signatures` metric untouched.
///
/// This gives the `Infra` arm of `report_proposer_preferences_collection_failure` its first
/// coverage and pins the classification boundary: the zero delta on
/// `insufficient_partial_signatures` proves an infra failure does not drift into the
/// observation-divergence bucket (whose value is an upper bound on the true divergence rate, so a
/// leak would silently inflate it).
#[tokio::test(flavor = "multi_thread")]
async fn proposer_preferences_infra_failure_increments_infra_metric() {
    // Reads the same global prometheus labels as the other failure test, so it joins the same
    // serialization; the cross-label zero-delta reads are only reliable under this lock.
    let _guard = METRIC_TEST_LOCK.lock().await;

    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            // EmptySignature classifies as the infra failure class.
            collector_failure: Some(CollectionError::EmptySignature),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let preferences =
        create_proposer_preferences(STARTING_VALIDATOR_INDEX as u64, Slot::new(TEST_SLOT));

    let metric = crate::metrics::PROPOSER_PREFERENCES_RECONSTRUCTION_FAILURES
        .as_ref()
        .expect("metric should be created");
    let infra_counter =
        metric.with_label_values(&[crate::metrics::PROPOSER_PREFERENCES_FAILURE_INFRA]);
    let insufficient_counter = metric.with_label_values(&[
        crate::metrics::PROPOSER_PREFERENCES_FAILURE_INSUFFICIENT_PARTIAL_SIGNATURES,
    ]);
    let infra_before = infra_counter.get();
    let insufficient_before = insufficient_counter.get();

    // Act
    let result = harness
        .validator_store
        .sign_proposer_preferences(pubkey, preferences)
        .await;

    // Assert
    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::SignatureCollectionFailed(CollectionError::EmptySignature)
            ))
        ),
        "expected EmptySignature surfaced as SignatureCollectionFailed, got: {result:?}"
    );
    assert_eq!(
        infra_counter.get() - infra_before,
        1,
        "EmptySignature should increment the infra reconstruction-failure metric once"
    );
    // The zero delta pins the classification boundary: an infra variant drifting into the
    // insufficient_partial_signatures bucket would silently inflate the divergence estimate.
    assert_eq!(
        insufficient_counter.get() - insufficient_before,
        0,
        "infra failures must not leak into the insufficient_partial_signatures divergence metric"
    );
}

/// A collection that never reaches quorum must fail per-validator with a *bounded* timeout and must
/// never hang the caller indefinitely (issue #1063 AC7). The mock collector captures the call and
/// then returns a future that never resolves, so the only way `sign_proposer_preferences` can return
/// is the production `tokio::time::timeout` elapsing after
/// `spec.get_slot_duration() * PROPOSER_PREFERENCES_COLLECTION_TIMEOUT_SLOTS` (= 24s under the
/// harness's mainnet spec). On elapse it synthesizes
/// `CollectionError::CollectionTimeout`, which classifies as the NoSignature bucket and increments
/// the `insufficient_partial_signatures` reconstruction-failure metric.
///
/// Falsifiability guard (intrinsic, no extra assertion needed): were the production
/// `tokio::time::timeout` removed, this test would hang forever awaiting the pending collector
/// future instead of returning `CollectionTimeout`. Completing at all — and returning the timeout
/// error — is exactly the "never hangs the caller" behavior AC7 requires.
///
/// Timing: `#[tokio::test(start_paused = true)]` runs on the current-thread runtime with a paused,
/// auto-advancing clock. Neither the harness constructor nor the `sign_proposer_preferences` path
/// spawns a background task that keeps the runtime busy, so once the call awaits the timeout the
/// runtime goes idle and tokio auto-advances virtual time straight to the 24s deadline. The 24s
/// therefore elapse in ~0 real time (verified via a wall-clock guard on the suite run), so no manual
/// `tokio::time::advance` is required.
#[tokio::test(start_paused = true)]
async fn proposer_preferences_no_quorum_hits_bounded_timeout() {
    // Reads the same global prometheus labels as the other failure tests, so it joins the same
    // serialization; the cross-label delta reads are only reliable under this lock.
    let _guard = METRIC_TEST_LOCK.lock().await;

    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            // The collector captures the call and then never resolves, so only the production
            // collection timeout can unblock the caller.
            collector_hangs: true,
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let preferences =
        create_proposer_preferences(STARTING_VALIDATOR_INDEX as u64, Slot::new(TEST_SLOT));

    let metric = crate::metrics::PROPOSER_PREFERENCES_RECONSTRUCTION_FAILURES
        .as_ref()
        .expect("metric should be created");
    let insufficient_counter = metric.with_label_values(&[
        crate::metrics::PROPOSER_PREFERENCES_FAILURE_INSUFFICIENT_PARTIAL_SIGNATURES,
    ]);
    let infra_counter =
        metric.with_label_values(&[crate::metrics::PROPOSER_PREFERENCES_FAILURE_INFRA]);
    let insufficient_before = insufficient_counter.get();
    let infra_before = infra_counter.get();

    // Act
    let result = harness
        .validator_store
        .sign_proposer_preferences(pubkey, preferences)
        .await;

    // Assert
    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::SignatureCollectionFailed(CollectionError::CollectionTimeout)
            ))
        ),
        "a no-quorum collection must surface CollectionTimeout via the bounded collection timeout, \
         got: {result:?}"
    );
    assert_eq!(
        insufficient_counter.get() - insufficient_before,
        1,
        "CollectionTimeout should increment the insufficient_partial_signatures reconstruction-failure \
         metric once (NoSignature bucket)"
    );
    // Pins the classification boundary: a bounded-timeout no-quorum is a NoSignature-class failure
    // and must NOT be attributed to the infra bucket.
    assert_eq!(
        infra_counter.get() - infra_before,
        0,
        "CollectionTimeout must not leak into the infra reconstruction-failure metric"
    );
    // The collection attempt must have started (the call was captured) before the collector hung,
    // proving the timeout wrapped an in-flight collection rather than short-circuiting earlier.
    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        1,
        "exactly one sign_and_collect call should have been captured before the collector hung"
    );
}

// ==================== Slashing-protection tests ====================

/// `sign_proposer_preferences` succeeds with slashing protection enabled, proving the path never
/// consults the slashing DB.
///
/// Tripwire mechanism: the harness slashing DB is created empty and no validator is ever
/// registered in it, so any slashing-protection check would fail for an unregistered validator. If
/// such a check were ever added to this code path, this call would flip from Ok to Err, which makes
/// the success assertion a real behavioral assertion rather than a tautology. No metric lock is
/// needed: this test reads no global prometheus labels.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_preferences_does_not_touch_slashing_db() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            collector_failure: None,
            disable_slashing_protection: false,
            ..Default::default()
        },
    );
    let preferences =
        create_proposer_preferences(STARTING_VALIDATOR_INDEX as u64, Slot::new(TEST_SLOT));

    // Act
    let result = harness
        .validator_store
        .sign_proposer_preferences(pubkey, preferences.clone())
        .await;

    // Assert
    let signed = result.expect("signing should succeed despite slashing protection being enabled");
    assert_eq!(
        signed.message, preferences,
        "signed message should echo the input preferences unchanged"
    );
}
