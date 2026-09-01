//! Envelope-signing duty tests (SIP-94 §6, disseminate-and-sign).

use std::sync::LazyLock;

use bls::{FixedBytesExtended, PublicKeyBytes};
use eth2::types::FullBlockContents;
use signature_collector::CollectionError;
use slashing_protection::Safe;
use ssv_types::{
    OperatorId, consensus::BlindedExecutionPayloadEnvelope, dissemination::EnvelopeDissemination,
    msgid::Role, partial_sig::PartialSignatureKind,
};
use ssz::Encode;
use ssz_types::VariableList;
use tree_hash::TreeHash;
use types::{
    BeaconBlock, BeaconBlockGloas, Domain, EmptyBlock, EthSpec, ExecutionPayloadEnvelope,
    ExecutionPayloadGloas, ExecutionRequestsGloas, Hash256, MainnetEthSpec,
    SignedExecutionPayloadEnvelope, SignedRoot, Slot, consts::gloas::BUILDER_INDEX_SELF_BUILD,
};
use validator_store::{UnsignedBlock, ValidatorStore};

use super::common::*;
use crate::{DecidedBlockContext, Error, SpecificError};

/// Serializes every test that records an envelope outcome: the labels live in the global
/// prometheus registry, so concurrent recordings would race the delta assertions. Tokio mutex
/// because the guard is held across awaits.
static METRIC_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

// ==================== Fixtures and helpers ====================

fn test_operator_ids() -> [OperatorId; 4] {
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)]
}

/// The block root envelope tests bind envelopes to.
fn test_decided_root() -> Hash256 {
    Hash256::from_low_u64_be(0xdec1)
}

/// Builds a Gloas self-build envelope at `TEST_SLOT` bound to `beacon_block_root`.
fn self_build_envelope(beacon_block_root: Hash256) -> ExecutionPayloadEnvelope<MainnetEthSpec> {
    ExecutionPayloadEnvelope {
        payload: ExecutionPayloadGloas::<MainnetEthSpec> {
            slot_number: Slot::new(TEST_SLOT),
            ..Default::default()
        },
        execution_requests: ExecutionRequestsGloas::<MainnetEthSpec>::default(),
        builder_index: BUILDER_INDEX_SELF_BUILD,
        beacon_block_root,
        parent_beacon_block_root: Hash256::from_low_u64_be(0x1111),
    }
}

/// Derives the decided-block context whose bindings all match `envelope`.
fn context_for(
    envelope: &ExecutionPayloadEnvelope<MainnetEthSpec>,
    built_locally: bool,
) -> DecidedBlockContext {
    DecidedBlockContext {
        beacon_block_root: envelope.beacon_block_root,
        parent_block_root: envelope.parent_beacon_block_root,
        execution_requests_root: envelope.execution_requests.tree_hash_root(),
        builder_index: envelope.builder_index,
        block_hash: envelope.payload.block_hash,
        built_locally,
    }
}

/// Records `context` for the validator at `TEST_SLOT`.
fn seed_context(
    harness: &ValidatorStoreTestHarness,
    pubkey: PublicKeyBytes,
    context: DecidedBlockContext,
) {
    harness
        .validator_store
        .record_decided_block_context(pubkey, Slot::new(TEST_SLOT), context)
        .expect("seeding the decided context must succeed");
}

/// Inserts the blinded form of `envelope` into the harness dissemination store, standing in
/// for the message receiver, and returns the inserted blinded envelope.
fn insert_dissemination(
    harness: &ValidatorStoreTestHarness,
    pubkey: PublicKeyBytes,
    envelope: &ExecutionPayloadEnvelope<MainnetEthSpec>,
) -> BlindedExecutionPayloadEnvelope<MainnetEthSpec> {
    let blinded = BlindedExecutionPayloadEnvelope::from_full(envelope);
    harness.dissemination_store.insert(
        pubkey,
        EnvelopeDissemination {
            slot: Slot::new(TEST_SLOT),
            envelope: VariableList::new(blinded.as_ssz_bytes())
                .expect("blinded envelope bytes should fit"),
        },
    );
    blinded
}

/// A single-validator committee over `test_operator_ids()` and its validator's public key.
fn single_validator_committee() -> (CommitteeSetup, PublicKeyBytes) {
    let committee = create_committee_setup(&test_operator_ids(), 1, 5);
    let pubkey = committee.validators[0].public_key;
    (committee, pubkey)
}

/// Default envelope-test options: Gloas at genesis, slashing protection disabled.
fn gloas_options() -> HarnessOptions {
    HarnessOptions {
        spec: gloas_at_genesis_spec(),
        disable_slashing_protection: true,
        ..Default::default()
    }
}

/// Builds a single-validator harness owned by `OperatorId(1)` with the given options.
fn harness_with_options(options: HarnessOptions) -> (ValidatorStoreTestHarness, PublicKeyBytes) {
    let (committee, pubkey) = single_validator_committee();
    let harness =
        ValidatorStoreTestHarness::new_with_options(vec![committee], OperatorId(1), options);
    (harness, pubkey)
}

/// Builds the default Gloas harness most envelope tests use.
fn gloas_harness() -> (ValidatorStoreTestHarness, PublicKeyBytes) {
    harness_with_options(gloas_options())
}

/// The `Domain::BeaconBuilder` domain hash at `TEST_SLOT`'s epoch, recomputed from the spec
/// so tests do not depend on the store's own fork-selection logic.
fn envelope_domain_hash(harness: &ValidatorStoreTestHarness) -> Hash256 {
    let spec = &harness.spec;
    let epoch = Slot::new(TEST_SLOT).epoch(MainnetEthSpec::slots_per_epoch());
    spec.get_domain(
        epoch,
        Domain::BeaconBuilder,
        &spec.fork_at_epoch(epoch),
        harness.genesis_validators_root,
    )
}

/// Drives the envelope duty under test against the store.
async fn sign_envelope(
    harness: &ValidatorStoreTestHarness,
    pubkey: PublicKeyBytes,
    envelope: ExecutionPayloadEnvelope<MainnetEthSpec>,
) -> Result<SignedExecutionPayloadEnvelope<MainnetEthSpec>, Error> {
    harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await
}

/// Before-values of the three envelope outcome labels, taken under `METRIC_TEST_LOCK`.
struct OutcomeCounters {
    published: crate::metrics::IntCounter,
    not_built_locally: crate::metrics::IntCounter,
    failed: crate::metrics::IntCounter,
    published_before: u64,
    not_built_locally_before: u64,
    failed_before: u64,
}

impl OutcomeCounters {
    fn snapshot() -> Self {
        let metric = crate::metrics::ENVELOPE_SIGNING_OUTCOMES
            .as_ref()
            .expect("metric should be created");
        let published = metric.with_label_values(&[crate::metrics::ENVELOPE_OUTCOME_PUBLISHED]);
        let not_built_locally =
            metric.with_label_values(&[crate::metrics::ENVELOPE_OUTCOME_NOT_BUILT_LOCALLY]);
        let failed = metric.with_label_values(&[crate::metrics::ENVELOPE_OUTCOME_FAILED]);
        Self {
            published_before: published.get(),
            not_built_locally_before: not_built_locally.get(),
            failed_before: failed.get(),
            published,
            not_built_locally,
            failed,
        }
    }

    /// Asserts the delta of every outcome label since the snapshot.
    fn assert_deltas(&self, published: u64, not_built_locally: u64, failed: u64) {
        assert_eq!(
            self.published.get() - self.published_before,
            published,
            "unexpected delta on the published outcome label"
        );
        assert_eq!(
            self.not_built_locally.get() - self.not_built_locally_before,
            not_built_locally,
            "unexpected delta on the not_built_locally outcome label"
        );
        assert_eq!(
            self.failed.get() - self.failed_before,
            failed,
            "unexpected delta on the failed outcome label"
        );
    }
}

/// Asserts that the duty stopped before any outward action: no dissemination broadcast and no
/// signature collection.
fn assert_no_outward_action(harness: &ValidatorStoreTestHarness, context: &str) {
    assert!(
        harness.captured_disseminations.lock().is_empty(),
        "no dissemination may be broadcast {context}"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen {context}"
    );
}

// ==================== Builder path ====================

/// The builder operator disseminates its blinded envelope, signs, and returns the envelope.
#[tokio::test(flavor = "multi_thread")]
async fn builder_disseminates_signs_and_publishes() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, true));

    let signed = sign_envelope(&harness, pubkey, envelope.clone())
        .await
        .expect("the builder path must sign successfully");

    assert_eq!(
        signed.message, envelope,
        "the returned message must be the input envelope unchanged"
    );
    let disseminations = harness.captured_disseminations.lock();
    assert_eq!(
        disseminations.len(),
        1,
        "the builder must broadcast exactly one dissemination"
    );
    assert_eq!(
        disseminations[0].validator_pubkey, pubkey,
        "the dissemination must carry the duty validator"
    );
    assert_eq!(
        disseminations[0].committee_id,
        ssv_types::CommitteeId::from(test_operator_ids().to_vec()),
        "the dissemination must route to the cluster's committee"
    );
    let sent = &disseminations[0].dissemination;
    assert_eq!(sent.slot, Slot::new(TEST_SLOT));
    assert_eq!(
        sent.blinded_envelope::<MainnetEthSpec>()
            .expect("the broadcast bytes must decode"),
        BlindedExecutionPayloadEnvelope::from_full(&envelope),
        "the broadcast must carry the blinded form of the local envelope"
    );
}

/// Exactly one collection happens, over the blinded envelope root under
/// `Domain::BeaconBuilder`, with the `Envelope` partial-signature kind.
#[tokio::test(flavor = "multi_thread")]
async fn signing_root_is_the_blinded_envelope_root_under_beacon_builder_domain() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, true));

    let domain_hash = envelope_domain_hash(&harness);
    let expected_root =
        BlindedExecutionPayloadEnvelope::from_full(&envelope).signing_root(domain_hash);

    sign_envelope(&harness, pubkey, envelope)
        .await
        .expect("the envelope duty must sign successfully");

    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        1,
        "exactly one signature collection must happen"
    );
    assert_eq!(
        captured[0].signing_root, expected_root,
        "the signing root must be the blinded envelope root under Domain::BeaconBuilder"
    );
    assert_eq!(
        captured[0].metadata.kind,
        PartialSignatureKind::Envelope,
        "the partial signature kind must be Envelope"
    );
    assert_eq!(
        captured[0].metadata.role,
        Role::EnvelopeProposer,
        "the collection role must be EnvelopeProposer"
    );
}

/// The envelope path succeeds with slashing protection enabled and writes no block-proposal
/// record at the duty slot: a first-time probe insertion afterwards is still accepted as safe.
#[tokio::test(flavor = "multi_thread")]
async fn envelope_signing_does_not_touch_the_slashing_db() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = harness_with_options(HarnessOptions {
        disable_slashing_protection: false,
        ..gloas_options()
    });
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, true));

    let signed = sign_envelope(&harness, pubkey, envelope.clone())
        .await
        .expect("the envelope duty must succeed despite slashing protection being enabled");

    assert_eq!(
        signed.message, envelope,
        "the returned message must be the input envelope unchanged"
    );
    // Any record left by the envelope path would surface here as `SameData` or a
    // double-proposal error instead of `Valid`.
    let first_time_probe = harness
        .slashing_protection
        .check_and_insert_block_signing_root(
            &pubkey,
            Slot::new(TEST_SLOT),
            Hash256::repeat_byte(0xEE).into(),
        );
    assert!(
        matches!(first_time_probe, Ok(Safe::Valid)),
        "no block-proposal record may exist at the duty slot after envelope signing, got {first_time_probe:?}"
    );
}

/// A builder whose local BN envelope carries a different execution block hash than the decided
/// bid must not disseminate or sign it.
#[tokio::test(flavor = "multi_thread")]
async fn builder_with_inconsistent_block_hash_broadcasts_nothing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    let mut context = context_for(&envelope, true);
    context.block_hash = types::ExecutionBlockHash::from_root(Hash256::repeat_byte(0xBB));
    seed_context(&harness, pubkey, context);
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeBuilderInconsistent { .. }
            ))
        ),
        "an inconsistent local envelope must be rejected with the dedicated error, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_no_outward_action(&harness, "for an inconsistent local envelope");
}

/// A builder whose local envelope fails a decision binding must not disseminate or sign it.
#[tokio::test(flavor = "multi_thread")]
async fn builder_with_binding_mismatch_broadcasts_nothing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    let mut context = context_for(&envelope, true);
    context.parent_block_root = Hash256::repeat_byte(0xCC);
    seed_context(&harness, pubkey, context);
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeBindingMismatch {
                    field: "parent_beacon_block_root"
                }
            ))
        ),
        "a binding mismatch must be rejected with the mismatching field, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_no_outward_action(&harness, "for a binding-mismatched local envelope");
}

// ==================== Non-builder path ====================

/// A non-builder signs the disseminated envelope's root and returns the sentinel error so the
/// caller never publishes.
#[tokio::test(flavor = "multi_thread")]
async fn non_builder_signs_disseminated_root_and_returns_sentinel() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    // The local envelope differs from the disseminated one in its payload content.
    let local_envelope = self_build_envelope(test_decided_root());
    let mut builder_envelope = local_envelope.clone();
    builder_envelope.payload.block_number = 42;
    seed_context(&harness, pubkey, context_for(&builder_envelope, false));
    let disseminated = insert_dissemination(&harness, pubkey, &builder_envelope);
    let counters = OutcomeCounters::snapshot();

    let domain_hash = envelope_domain_hash(&harness);

    let result = sign_envelope(&harness, pubkey, local_envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeNotBuiltLocally { .. }
            ))
        ),
        "a non-builder must return the sentinel after contributing, got {result:?}"
    );
    counters.assert_deltas(0, 1, 0);
    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        1,
        "the non-builder must still contribute its signature share"
    );
    assert_eq!(
        captured[0].signing_root,
        disseminated.signing_root(domain_hash),
        "the non-builder must sign the disseminated envelope's root, not its own"
    );
    assert!(
        harness.captured_disseminations.lock().is_empty(),
        "a non-builder must never broadcast a dissemination"
    );
}

/// Without a dissemination, the non-builder times out at the deadline having signed nothing.
#[tokio::test(start_paused = true)]
async fn non_builder_without_dissemination_times_out() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, false));
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::DisseminationTimeout { .. }
            ))
        ),
        "a missing dissemination must time out with the dedicated error, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_no_outward_action(&harness, "when no dissemination arrives");
}

/// A disseminated envelope that fails a decision binding is never signed.
#[tokio::test(flavor = "multi_thread")]
async fn non_builder_rejects_binding_mismatched_dissemination() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let local_envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&local_envelope, false));
    // Disseminated envelope binds to a different beacon block root.
    let mut forged = local_envelope.clone();
    forged.beacon_block_root = Hash256::repeat_byte(0xDD);
    insert_dissemination(&harness, pubkey, &forged);
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, local_envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeBindingMismatch {
                    field: "beacon_block_root"
                }
            ))
        ),
        "a binding-mismatched dissemination must be rejected, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_no_outward_action(&harness, "for a binding-mismatched dissemination");
}

/// Disseminated bytes that do not decode as a blinded envelope are never signed.
#[tokio::test(flavor = "multi_thread")]
async fn non_builder_rejects_undecodable_dissemination() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, false));
    harness.dissemination_store.insert(
        pubkey,
        EnvelopeDissemination {
            slot: Slot::new(TEST_SLOT),
            envelope: VariableList::new(vec![0xFF; 3]).expect("garbage bytes should fit"),
        },
    );
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::DisseminationUndecodable { .. }
            ))
        ),
        "undecodable disseminated bytes must be rejected, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_no_outward_action(&harness, "for an undecodable dissemination");
}

// ==================== Gate tests ====================

/// A pre-Gloas slot is rejected before any outward action.
#[tokio::test(flavor = "multi_thread")]
async fn pre_gloas_slot_is_rejected_before_signing() {
    let (harness, pubkey) = harness_with_options(HarnessOptions {
        spec: electra_at_genesis_spec(),
        ..Default::default()
    });
    let envelope = self_build_envelope(test_decided_root());

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeBeforeGloas { .. }
            ))
        ),
        "a pre-Gloas envelope must be rejected with the dedicated error, got {result:?}"
    );
    assert_no_outward_action(&harness, "for a pre-Gloas envelope");
}

/// A non-self-build envelope is rejected before any outward action.
#[tokio::test(flavor = "multi_thread")]
async fn non_self_build_envelope_is_rejected_before_signing() {
    let (harness, pubkey) = gloas_harness();
    let mut envelope = self_build_envelope(test_decided_root());
    envelope.builder_index = 7;

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(SpecificError::EnvelopeNotSelfBuild {
                builder_index: 7
            }))
        ),
        "a non-self-build envelope must be rejected with the dedicated error, got {result:?}"
    );
    assert_no_outward_action(&harness, "for a non-self-build envelope");
}

/// A future-slot envelope is rejected before any outward action.
#[tokio::test(flavor = "multi_thread")]
async fn future_slot_envelope_is_rejected_before_signing() {
    let (harness, pubkey) = gloas_harness();
    let mut envelope = self_build_envelope(test_decided_root());
    envelope.payload.slot_number = Slot::new(TEST_SLOT + 1);

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(result, Err(Error::GreaterThanCurrentSlot { .. })),
        "a future-slot envelope must be rejected with the dedicated error, got {result:?}"
    );
    assert_no_outward_action(&harness, "for a future-slot envelope");
}

/// An envelope without a recorded decided context is rejected before any outward action.
#[tokio::test(flavor = "multi_thread")]
async fn missing_decided_context_is_rejected_before_signing() {
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::DecidedRootUnavailable { .. }
            ))
        ),
        "an envelope without a decided context must be rejected with the dedicated error, got {result:?}"
    );
    assert_no_outward_action(&harness, "without a decided context");
}

/// A duty starting past the payload-due mark (50% of the slot) is rejected before any outward
/// action, builder or not.
#[tokio::test(flavor = "multi_thread")]
async fn duty_past_the_payload_due_deadline_is_rejected() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, true));
    // Position the clock 7s into the 12s slot, past the 6s payload-due mark. `start_of` is a
    // `SlotClock` trait method, so bring the trait into scope locally.
    use slot_clock::SlotClock as _;
    let slot_start = harness
        .slot_clock
        .start_of(Slot::new(TEST_SLOT))
        .expect("slot start must exist");
    harness
        .slot_clock
        .set_current_time(slot_start + std::time::Duration::from_secs(7));
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeDeadlinePassed { .. }
            ))
        ),
        "a duty past the payload-due mark must be rejected, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_no_outward_action(&harness, "past the payload-due deadline");
}

// ==================== Metrics and failure tests ====================

/// The happy path increments only the `published` label.
#[tokio::test(flavor = "multi_thread")]
async fn published_outcome_increments_only_the_published_label() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, true));
    let counters = OutcomeCounters::snapshot();

    sign_envelope(&harness, pubkey, envelope)
        .await
        .expect("the happy-path envelope duty must sign successfully");

    counters.assert_deltas(1, 0, 0);
}

/// A signature-collection failure increments the `failed` label.
#[tokio::test(flavor = "multi_thread")]
async fn collection_failure_increments_the_failed_label() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = harness_with_options(HarnessOptions {
        collector_failure: Some(CollectionError::EmptySignature),
        ..gloas_options()
    });
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, true));
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::SignatureCollectionFailed(CollectionError::EmptySignature)
            ))
        ),
        "a collection failure must surface as SignatureCollectionFailed, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
}

/// A failed dissemination broadcast surfaces the dedicated error, increments `failed`, and
/// never starts signature collection.
#[tokio::test(flavor = "multi_thread")]
async fn broadcast_failure_increments_failed_and_signs_nothing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = harness_with_options(HarnessOptions {
        dissemination_failure: Some(CollectionError::DisseminationSendFailed(
            "sender closed".to_string(),
        )),
        ..gloas_options()
    });
    let envelope = self_build_envelope(test_decided_root());
    seed_context(&harness, pubkey, context_for(&envelope, true));
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::DisseminationBroadcastFailed(_)
            ))
        ),
        "a broadcast failure must surface as DisseminationBroadcastFailed, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen after a failed broadcast"
    );
}

// ==================== End-to-end tests ====================

/// End to end: `sign_block` records the decided context (with builder provenance), then a
/// matching envelope disseminates and signs through the builder path.
#[tokio::test(flavor = "multi_thread")]
async fn sign_block_then_matching_envelope_succeeds() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    // `EmptyBlock::empty` fixes the slot to `spec.genesis_slot`, so the slot is set on the
    // inner struct before wrapping. The bid commitments are aligned with the envelope the
    // test presents afterwards, since the write site now records them for validation.
    let mut gloas_block = BeaconBlockGloas::<MainnetEthSpec>::empty(&harness.spec);
    gloas_block.slot = Slot::new(TEST_SLOT);
    {
        let bid = &mut gloas_block.body.signed_execution_payload_bid.message;
        // A self-build block: the empty fixture's zeroed bid must carry the sentinel and the
        // commitments the presented envelope will bind to.
        bid.builder_index = BUILDER_INDEX_SELF_BUILD;
        bid.execution_requests_root =
            ExecutionRequestsGloas::<MainnetEthSpec>::default().tree_hash_root();
    }
    let bid = &gloas_block.body.signed_execution_payload_bid.message;
    let parent_block_root = bid.parent_block_root;
    let block = BeaconBlock::Gloas(gloas_block);
    let mut envelope = self_build_envelope(block.canonical_root());
    envelope.parent_beacon_block_root = parent_block_root;

    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(TEST_SLOT),
        )
        .await
        .expect("the Gloas block duty must sign successfully");
    let signed = sign_envelope(&harness, pubkey, envelope.clone())
        .await
        .expect("an envelope matching the recorded context must sign through the builder path");

    assert_eq!(
        signed.message, envelope,
        "the returned message must be the input envelope unchanged"
    );
    assert_eq!(
        harness.captured_disseminations.lock().len(),
        1,
        "the builder provenance recorded by sign_block must drive a dissemination"
    );
}
