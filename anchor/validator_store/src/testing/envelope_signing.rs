//! Envelope-signing duty tests (SIP-94 §6, disseminate-and-sign).

use std::{sync::LazyLock, time::Duration};

use bls::{FixedBytesExtended, PublicKeyBytes};
use eth2::types::FullBlockContents;
use signature_collector::CollectionError;
use slashing_protection::Safe;
use ssv_types::{
    OperatorId,
    consensus::{
        BEACON_ROLE_PROPOSER, BlindedExecutionPayloadEnvelope, DataVersion, ProposerConsensusData,
        ValidatorDuty,
    },
    dissemination::EnvelopeDissemination,
    msgid::Role,
    partial_sig::PartialSignatureKind,
};
use ssz::Encode;
use ssz_types::VariableList;
use tree_hash::TreeHash;
use types::{
    BeaconBlock, BeaconBlockGloas, ChainSpec, Domain, EmptyBlock, EthSpec,
    ExecutionPayloadEnvelope, ExecutionPayloadGloas, ExecutionRequestsGloas, ForkName, Hash256,
    MainnetEthSpec, SignedExecutionPayloadEnvelope, SignedRoot, Slot,
    consts::gloas::BUILDER_INDEX_SELF_BUILD,
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

/// Runs the body of the detached non-builder task for the harness validator at `TEST_SLOT`,
/// awaiting it directly: the same code `sign_block` spawns, minus the executor.
async fn run_non_builder_task(
    harness: &ValidatorStoreTestHarness,
    pubkey: PublicKeyBytes,
    context: DecidedBlockContext,
) -> Result<(), Error> {
    let (validator, cluster) = harness.validator_store.get_validator_and_cluster(pubkey)?;
    harness
        .validator_store
        .clone()
        .sign_disseminated_envelope(validator, cluster, context, Slot::new(TEST_SLOT))
        .await
}

/// Number of captured signature collections of the envelope kind.
fn envelope_collection_count(harness: &ValidatorStoreTestHarness) -> usize {
    harness
        .captured_calls
        .lock()
        .iter()
        .filter(|call| call.metadata.kind == PartialSignatureKind::Envelope)
        .count()
}

/// Waits for the detached non-builder task to reach signature collection and returns the root
/// it signed. Bounded so a task that never signs fails the test instead of hanging it.
async fn wait_for_envelope_signing_root(harness: &ValidatorStoreTestHarness) -> Hash256 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let signed_root = harness
                .captured_calls
                .lock()
                .iter()
                .find(|call| call.metadata.kind == PartialSignatureKind::Envelope)
                .map(|call| call.signing_root);
            if let Some(root) = signed_root {
                return root;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the non-builder task must reach signature collection")
}

/// Gives spawned tasks a chance to run before a negative assertion: a wrongly spawned
/// non-builder task with a valid dissemination already stored would sign immediately.
async fn let_spawned_tasks_run() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// A Gloas block at `TEST_SLOT` whose bid names `builder_index` and commits to the default
/// execution requests, so `self_build_envelope(block.canonical_root())` binds to it.
fn gloas_block_with_bid(spec: &ChainSpec, builder_index: u64) -> BeaconBlock<MainnetEthSpec> {
    // `EmptyBlock::empty` fixes the slot to `spec.genesis_slot`, so the slot is set on the
    // inner struct before wrapping.
    let mut gloas_block = BeaconBlockGloas::<MainnetEthSpec>::empty(spec);
    gloas_block.slot = Slot::new(TEST_SLOT);
    let bid = &mut gloas_block.body.signed_execution_payload_bid.message;
    bid.builder_index = builder_index;
    bid.execution_requests_root =
        ExecutionRequestsGloas::<MainnetEthSpec>::default().tree_hash_root();
    BeaconBlock::Gloas(gloas_block)
}

/// The envelope for `block` that every decision binding accepts.
fn envelope_for_block(
    block: &BeaconBlock<MainnetEthSpec>,
) -> ExecutionPayloadEnvelope<MainnetEthSpec> {
    let mut envelope = self_build_envelope(block.canonical_root());
    envelope.parent_beacon_block_root = block.parent_root();
    envelope
}

/// The consensus value a fixed-decision mock returns so `sign_block` decides `block` for the
/// harness validator regardless of the local proposal.
fn decided_consensus_data(
    committee: &CommitteeSetup,
    pubkey: PublicKeyBytes,
    block: &BeaconBlock<MainnetEthSpec>,
) -> ProposerConsensusData {
    let validator_index = committee.validators[0]
        .index
        .expect("the harness validator has a beacon index");
    ProposerConsensusData {
        duty: ValidatorDuty {
            r#type: BEACON_ROLE_PROPOSER,
            pub_key: pubkey,
            slot: Slot::new(TEST_SLOT),
            validator_index,
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        },
        version: DataVersion::from(ForkName::Gloas),
        data_ssz: VariableList::new(block.as_ssz_bytes()).expect("block bytes should fit"),
    }
}

/// Before-values of the three envelope outcome labels, taken under `METRIC_TEST_LOCK`.
struct OutcomeCounters {
    published: crate::metrics::IntCounter,
    not_built_locally: crate::metrics::IntCounter,
    failed: crate::metrics::IntCounter,
    external_build: crate::metrics::IntCounter,
    published_before: u64,
    not_built_locally_before: u64,
    failed_before: u64,
    external_build_before: u64,
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
        let external_build =
            metric.with_label_values(&[crate::metrics::ENVELOPE_OUTCOME_EXTERNAL_BUILD]);
        Self {
            published_before: published.get(),
            not_built_locally_before: not_built_locally.get(),
            failed_before: failed.get(),
            external_build_before: external_build.get(),
            published,
            not_built_locally,
            failed,
            external_build,
        }
    }

    /// Asserts the delta of every outcome label since the snapshot. `assert_deltas` pins
    /// the external_build label to zero; external-build tests use `assert_external_build`.
    fn assert_external_build(&self, external_build: u64) {
        assert_eq!(
            self.external_build.get() - self.external_build_before,
            external_build,
            "unexpected delta on the external_build outcome label"
        );
    }

    fn assert_deltas(&self, published: u64, not_built_locally: u64, failed: u64) {
        self.assert_external_build(0);
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

/// Lighthouse's envelope callback on a non-builder returns the delegated sentinel at once: the
/// share is signed by the task `sign_block` spawned, and the callback must neither wait for the
/// dissemination nor collect a second signature.
#[tokio::test(start_paused = true)]
async fn non_builder_callback_returns_delegated_sentinel_without_signing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let local_envelope = self_build_envelope(test_decided_root());
    let mut builder_envelope = local_envelope.clone();
    builder_envelope.payload.block_number = 42;
    seed_context(&harness, pubkey, context_for(&builder_envelope, false));
    insert_dissemination(&harness, pubkey, &builder_envelope);
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, local_envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeNonBuilderDelegated { .. }
            ))
        ),
        "a non-builder callback must return the delegated sentinel, got {result:?}"
    );
    counters.assert_deltas(0, 0, 0);
    assert_no_outward_action(&harness, "from a non-builder callback");
}

/// The non-builder task signs the disseminated envelope's root, never its own, and broadcasts
/// no dissemination.
#[tokio::test(flavor = "multi_thread")]
async fn non_builder_task_signs_disseminated_root() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    // The builder's envelope differs from what this operator's own node built.
    let mut builder_envelope = self_build_envelope(test_decided_root());
    builder_envelope.payload.block_number = 42;
    let context = context_for(&builder_envelope, false);
    seed_context(&harness, pubkey, context);
    let disseminated = insert_dissemination(&harness, pubkey, &builder_envelope);
    let counters = OutcomeCounters::snapshot();

    let domain_hash = envelope_domain_hash(&harness);

    run_non_builder_task(&harness, pubkey, context)
        .await
        .expect("the non-builder task must contribute its share");

    counters.assert_deltas(0, 1, 0);
    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        1,
        "exactly one signature share is contributed"
    );
    assert_eq!(
        captured[0].signing_root,
        disseminated.signing_root(domain_hash),
        "the non-builder must sign the disseminated envelope's root"
    );
    assert!(
        harness.captured_disseminations.lock().is_empty(),
        "a non-builder must never broadcast a dissemination"
    );
}

/// Without a dissemination, the non-builder task times out at the deadline having signed
/// nothing.
#[tokio::test(start_paused = true)]
async fn non_builder_without_dissemination_times_out() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    let context = context_for(&envelope, false);
    seed_context(&harness, pubkey, context);
    let counters = OutcomeCounters::snapshot();

    let result = run_non_builder_task(&harness, pubkey, context).await;

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
    let context = context_for(&local_envelope, false);
    seed_context(&harness, pubkey, context);
    // Disseminated envelope binds to a different beacon block root.
    let mut forged = local_envelope.clone();
    forged.beacon_block_root = Hash256::repeat_byte(0xDD);
    insert_dissemination(&harness, pubkey, &forged);
    let counters = OutcomeCounters::snapshot();

    let result = run_non_builder_task(&harness, pubkey, context).await;

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
    let context = context_for(&envelope, false);
    seed_context(&harness, pubkey, context);
    harness.dissemination_store.insert(
        pubkey,
        EnvelopeDissemination {
            slot: Slot::new(TEST_SLOT),
            envelope: VariableList::new(vec![0xFF; 3]).expect("garbage bytes should fit"),
        },
    );
    let counters = OutcomeCounters::snapshot();

    let result = run_non_builder_task(&harness, pubkey, context).await;

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

/// A decided context that committed to an external builder's bid short-circuits the duty
/// before the non-builder path can wait for a dissemination that will never arrive: the
/// runner returns the dedicated no-op sentinel immediately, with no outward action. The
/// local BN's envelope is self-build here (the mixed case: this operator produced a
/// self-build candidate, but consensus decided another operator's external-bid block).
#[tokio::test(start_paused = true)]
async fn external_build_decision_short_circuits_before_waiting() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let counters = OutcomeCounters::snapshot();
    let envelope = self_build_envelope(test_decided_root());
    let mut context = context_for(&envelope, false);
    context.builder_index = 7;
    seed_context(&harness, pubkey, context);

    // With a paused clock, a regression back into the dissemination wait would hang the
    // test rather than pass it: nothing advances time and no dissemination is inserted.
    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(SpecificError::EnvelopeExternalBuild {
                builder_index: 7
            }))
        ),
        "an external-build decision must return the no-op sentinel, got {result:?}"
    );
    assert_no_outward_action(&harness, "for an external-build decision");
    counters.assert_external_build(1);
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

/// Mixed bid, the case Lighthouse's callback cannot serve: this operator's own node took an
/// external builder's bid, the cluster decided another operator's self-build block. `sign_block`
/// must spawn the non-builder task, which signs the disseminated envelope's root once it arrives.
/// A later callback for the slot returns the delegated sentinel and adds no second share.
#[tokio::test(flavor = "multi_thread")]
async fn sign_block_with_another_operators_self_build_block_signs_its_envelope() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (committee, pubkey) = single_validator_committee();
    let spec = gloas_at_genesis_spec();
    let decided_block = gloas_block_with_bid(&spec, BUILDER_INDEX_SELF_BUILD);
    let decided = decided_consensus_data(&committee, pubkey, &decided_block);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            decider: MockConsensusDecider::fixed_after_barrier(&decided, 1),
            ..gloas_options()
        },
    );
    let local_block = gloas_block_with_bid(&spec, 7);
    assert_ne!(
        local_block.canonical_root(),
        decided_block.canonical_root(),
        "the fixture must model a decided block this operator did not build"
    );
    let builder_envelope = envelope_for_block(&decided_block);
    let domain_hash = envelope_domain_hash(&harness);

    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(local_block)),
            Slot::new(TEST_SLOT),
        )
        .await
        .expect("the Gloas block duty must sign successfully");
    // The builder operator's dissemination arrives after the block decided.
    let disseminated = insert_dissemination(&harness, pubkey, &builder_envelope);

    let signed_root = wait_for_envelope_signing_root(&harness).await;

    assert_eq!(
        signed_root,
        disseminated.signing_root(domain_hash),
        "the spawned task must sign the disseminated envelope's root"
    );
    assert!(
        harness.captured_disseminations.lock().is_empty(),
        "a non-builder must never broadcast a dissemination"
    );

    // Lighthouse's callback for the same slot: nothing to publish, no second share.
    let result = sign_envelope(&harness, pubkey, envelope_for_block(&decided_block)).await;
    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeNonBuilderDelegated { .. }
            ))
        ),
        "the callback on a non-builder must return the delegated sentinel, got {result:?}"
    );
    assert_eq!(
        envelope_collection_count(&harness),
        1,
        "the callback must not collect a second envelope share"
    );
}

/// The builder operator signs from Lighthouse's callback only: `sign_block` on a block this
/// operator built spawns no non-builder task. A valid dissemination is stored up front so a
/// wrongly spawned task would sign immediately and be caught.
#[tokio::test(flavor = "multi_thread")]
async fn sign_block_as_builder_spawns_no_non_builder_task() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let block = gloas_block_with_bid(&harness.spec, BUILDER_INDEX_SELF_BUILD);
    insert_dissemination(&harness, pubkey, &envelope_for_block(&block));

    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(TEST_SLOT),
        )
        .await
        .expect("the Gloas block duty must sign successfully");
    let_spawned_tasks_run().await;

    assert_eq!(
        envelope_collection_count(&harness),
        0,
        "the builder must not sign its envelope before Lighthouse's callback"
    );
}

/// A decided block that committed to an external builder's bid has no self-build envelope duty,
/// so `sign_block` spawns nothing even though this operator did not build the block.
#[tokio::test(flavor = "multi_thread")]
async fn sign_block_with_external_build_decision_spawns_no_non_builder_task() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (committee, pubkey) = single_validator_committee();
    let spec = gloas_at_genesis_spec();
    let decided_block = gloas_block_with_bid(&spec, 7);
    let decided = decided_consensus_data(&committee, pubkey, &decided_block);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            decider: MockConsensusDecider::fixed_after_barrier(&decided, 1),
            ..gloas_options()
        },
    );
    let local_block = gloas_block_with_bid(&spec, BUILDER_INDEX_SELF_BUILD);
    insert_dissemination(&harness, pubkey, &envelope_for_block(&decided_block));

    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(local_block)),
            Slot::new(TEST_SLOT),
        )
        .await
        .expect("the Gloas block duty must sign successfully");
    let_spawned_tasks_run().await;

    assert_eq!(
        envelope_collection_count(&harness),
        0,
        "an external-build decision carries no self-build envelope duty"
    );
}

/// Lighthouse can invoke `sign_block` twice for one slot: a second block-service notification
/// for the same slot was observed on a devnet. The repeat must fail slashing protection inside
/// `sign_abstract_block` as `SameData` before `sign_block` reaches the non-builder spawn, so only
/// the first call's task signs. This pins the spawn's placement after `sign_abstract_block`: the
/// other spawn-path tests disable slashing protection and call `sign_block` once, so a spawn
/// moved above the slashing check would pass them and double-sign on the devnet.
#[tokio::test(flavor = "multi_thread")]
async fn repeated_sign_block_for_the_same_slot_spawns_one_non_builder_task() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (committee, pubkey) = single_validator_committee();
    let spec = gloas_at_genesis_spec();
    let decided_block = gloas_block_with_bid(&spec, BUILDER_INDEX_SELF_BUILD);
    let decided = decided_consensus_data(&committee, pubkey, &decided_block);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            decider: MockConsensusDecider::fixed_after_barrier(&decided, 1),
            disable_slashing_protection: false,
            ..gloas_options()
        },
    );
    let local_block = gloas_block_with_bid(&spec, 7);
    assert_ne!(
        local_block.canonical_root(),
        decided_block.canonical_root(),
        "the fixture must model a decided block this operator did not build"
    );
    // Stored up front so the legitimately spawned task signs immediately, and a wrongly spawned
    // second task would too.
    insert_dissemination(&harness, pubkey, &envelope_for_block(&decided_block));

    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(local_block.clone())),
            Slot::new(TEST_SLOT),
        )
        .await
        .expect("the first Gloas block duty must sign successfully");
    wait_for_envelope_signing_root(&harness).await;

    let repeat = harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(local_block)),
            Slot::new(TEST_SLOT),
        )
        .await;
    assert!(
        matches!(repeat, Err(Error::SameData)),
        "a repeated sign_block for the same slot must be rejected by slashing protection as SameData, got {repeat:?}"
    );
    let_spawned_tasks_run().await;

    assert_eq!(
        envelope_collection_count(&harness),
        1,
        "a repeated sign_block must not spawn a second non-builder task"
    );
    assert!(
        harness.captured_disseminations.lock().is_empty(),
        "a non-builder must never broadcast a dissemination"
    );
}
