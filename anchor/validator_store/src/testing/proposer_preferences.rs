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
use std::sync::LazyLock;

use bls::FixedBytesExtended;
use signature_collector::{CollectionError, SignatureRequester};
use ssv_types::{OperatorId, msgid::Role, partial_sig::PartialSignatureKind};
use types::{
    Address, ChainSpec, Domain, EthSpec, Hash256, MainnetEthSpec, ProposerPreferences, SignedRoot,
    Slot,
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

/// Independently recomputes the expected signing root for a `ProposerPreferences`, using the same
/// mainnet spec and zero `genesis_validators_root` the harness wires up. Pins that the domain is
/// `Domain::ProposerPreferences` and that its epoch is derived from `proposal_slot`.
fn expected_signing_root(preferences: &ProposerPreferences) -> Hash256 {
    let spec = ChainSpec::mainnet();
    let epoch = preferences
        .proposal_slot
        .epoch(MainnetEthSpec::slots_per_epoch());
    let domain = spec.get_domain(
        epoch,
        Domain::ProposerPreferences,
        &spec.fork_at_epoch(epoch),
        Hash256::zero(),
    );
    preferences.signing_root(domain)
}

// ==================== Success / signing-root tests ====================

/// `sign_proposer_preferences` collects a single-validator signature and echoes the input back in
/// the resulting message, committing to the `ProposerPreferences` under the `ProposerPreferences`
/// domain keyed by the proposal slot's epoch.
///
/// Uses a lookahead `proposal_slot` (LOOKAHEAD_EPOCHS ahead of the send slot) so the recomputed
/// domain epoch is `epoch(proposal_slot)` for a *future* slot, not the send slot's epoch: this
/// exercises acceptance criterion "domain epoch equals `epoch(proposal_slot)`" for a real
/// lookahead. It also asserts the broadcast `PartialSignatureMessages.slot` (captured via the
/// collector call) equals `preferences.proposal_slot`, the SIP-94 §5/§7 wire-slot invariant.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_preferences_reconstruction_threshold() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);
    // A future proposal slot so `epoch(proposal_slot)` differs from the send slot's epoch; the
    // independent root recompute then pins the domain epoch to the proposal slot, not the clock.
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
    // The broadcast `PartialSignatureMessages.slot` (captured off the collector call) must equal
    // `proposal_slot`: SIP-94 §5/§7 pins the wire slot to the slot being proposed, and #1062
    // validates proposer assignment against it.
    assert_eq!(
        call.metadata.slot, preferences.proposal_slot,
        "the broadcast partial-signature slot should equal the proposal slot"
    );

    // Recompute the root independently to lock the ProposerPreferences-specific signing decisions
    // that no other test covers: the `Domain::ProposerPreferences` choice, the epoch derived from
    // `proposal_slot` (here a future slot, so this pins epoch keying to the proposal, not the
    // clock), and the signed object being the `ProposerPreferences` itself. A regression to a
    // different domain or a different epoch key fails here.
    let expected_root = expected_signing_root(&preferences);
    assert_eq!(
        call.signing_root, expected_root,
        "signing root should commit to the proposer preferences under the ProposerPreferences domain"
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

    // The envelope slot must be the future `proposal_slot` (SIP-94 §5/§7), so the on-wire
    // `PartialSignatureMessages.slot` equals the slot the preference targets. This is what peers
    // validate proposer assignment against (#1062) and what keeps the collector alive until
    // `proposal_slot + 1`.
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
/// `signing_root_divergence` reconstruction-failure metric exactly once.
///
/// Log-field assertion path: the codebase has no tracing/log-capture utility (no `tracing-test`
/// dependency, no `logs_contain`/subscriber-capture helper), so we deliberately do NOT build
/// fragile tracing infrastructure. The warn-log fields (validator index, proposal slot,
/// `target_gas_limit`, `dependent_root`, signing root) and the `signing_root_divergence` metric
/// increment are emitted by the *same* `NoSignature` match arm in
/// `report_proposer_preferences_collection_failure`, so the metric increment proves that warn
/// branch executed; the log-field assertion is covered indirectly.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_preferences_signing_root_divergence_warns_and_metrics() {
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
            // QueueClosedError classifies as the NoSignature / signing-root-divergence class.
            collector_failure: Some(CollectionError::QueueClosedError),
            disable_slashing_protection: true,
        },
    );
    let preferences =
        create_proposer_preferences(STARTING_VALIDATOR_INDEX as u64, Slot::new(TEST_SLOT));

    // The metric lives in the global prometheus registry shared by every test in the process, so
    // we assert on the delta. The other metric test also touches this label, so the delta is only
    // reliable because both tests hold `METRIC_TEST_LOCK`; future failure tests must join that
    // serialization or use distinct labels.
    let divergence_counter = crate::metrics::PROPOSER_PREFERENCES_RECONSTRUCTION_FAILURES
        .as_ref()
        .expect("metric should be created")
        .with_label_values(&[crate::metrics::PROPOSER_PREFERENCES_FAILURE_SIGNING_ROOT_DIVERGENCE]);
    let count_before = divergence_counter.get();

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
        divergence_counter.get() - count_before,
        1,
        "QueueClosedError should increment the signing_root_divergence reconstruction-failure \
         metric once"
    );
}

/// The code classifies collection failures only into the coarse `signing_root_divergence` /
/// `infra` buckets; it never attributes a divergence to a specific input field. A
/// `target_gas_limit` or `dependent_root` mismatch is only observable as a signing-root split
/// (threshold-not-reached), which surfaces here as the same `QueueClosedError`.
///
/// This asserts the `signing_root_divergence` label increments while the per-input attribution
/// labels (`"target_gas_limit_divergence"`, `"dependent_root_divergence"`) stay at 0, proving the
/// code emits no per-input attribution reason.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_preferences_does_not_classify_remote_input_without_metadata() {
    // Touches the same global metric as the other failure test, so it joins the same
    // serialization.
    let _guard = METRIC_TEST_LOCK.lock().await;

    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            // A target_gas_limit / dependent_root mismatch is only observable as a signing-root
            // split, i.e. threshold-not-reached, which the collector surfaces as QueueClosedError.
            collector_failure: Some(CollectionError::QueueClosedError),
            disable_slashing_protection: true,
        },
    );
    let preferences =
        create_proposer_preferences(STARTING_VALIDATOR_INDEX as u64, Slot::new(TEST_SLOT));

    let metric = crate::metrics::PROPOSER_PREFERENCES_RECONSTRUCTION_FAILURES
        .as_ref()
        .expect("metric should be created");
    let divergence_counter = metric
        .with_label_values(&[crate::metrics::PROPOSER_PREFERENCES_FAILURE_SIGNING_ROOT_DIVERGENCE]);
    // Read the hypothetical per-input attribution labels directly; the code never emits them, so
    // they must stay at zero. Using literal strings (not consts) is deliberate: no such consts
    // exist because the production code never references these buckets.
    let target_gas_limit_attribution = metric.with_label_values(&["target_gas_limit_divergence"]);
    let dependent_root_attribution = metric.with_label_values(&["dependent_root_divergence"]);
    let divergence_before = divergence_counter.get();

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
        "expected QueueClosedError surfaced as SignatureCollectionFailed, got: {result:?}"
    );
    assert_eq!(
        divergence_counter.get() - divergence_before,
        1,
        "the coarse signing_root_divergence bucket should increment once"
    );
    // The per-input attribution buckets are never written by the production code: it cannot tell a
    // target_gas_limit split from a dependent_root split at the partial-signature wire, so it emits
    // no per-input reason.
    assert_eq!(
        target_gas_limit_attribution.get(),
        0,
        "code must not attribute divergence to a target_gas_limit mismatch"
    );
    assert_eq!(
        dependent_root_attribution.get(),
        0,
        "code must not attribute divergence to a dependent_root mismatch"
    );
}
