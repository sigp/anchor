//! Envelope-signing duty tests (SIP-94).

use std::sync::LazyLock;

use bls::{FixedBytesExtended, PublicKeyBytes};
use eth2::types::FullBlockContents;
use qbft::Completed;
use qbft_manager::{ConsensusDecider, EnvelopeProposerInstanceId, QbftError, TimeoutMode};
use signature_collector::CollectionError;
use slashing_protection::Safe;
use ssv_types::{
    IndexSet, OperatorId, ValidatorIndex,
    consensus::{
        BEACON_ROLE_ENVELOPE_PROPOSER, BlindedExecutionPayloadEnvelope, DataVersion,
        EnvelopeConsensusData, EnvelopeConsensusDataValidator, QbftDataValidator, ValidatorDuty,
    },
    msgid::Role,
    partial_sig::PartialSignatureKind,
};
use ssz::Encode;
use ssz_types::VariableList;
use tokio::time::Instant;
use types::{
    BeaconBlock, BeaconBlockGloas, Domain, EmptyBlock, EthSpec, ExecutionBlockHash,
    ExecutionPayloadEnvelope, ExecutionPayloadGloas, ExecutionRequestsGloas, ForkName, Hash256,
    MainnetEthSpec, SignedExecutionPayloadEnvelope, SignedRoot, Slot,
    consts::gloas::BUILDER_INDEX_SELF_BUILD,
};
use validator_store::{UnsignedBlock, ValidatorStore};

use super::common::*;
use crate::{DecidedBlockContext, Error, SpecificError};

/// Validator index the single-validator committee starts at.
const STARTING_VALIDATOR_INDEX: usize = 5;

/// Serializes every test that records an envelope outcome: the labels live in the global
/// prometheus registry, so concurrent recordings would race the delta assertions. Tokio mutex
/// because the guard is held across awaits.
static METRIC_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

// ==================== Fixtures and helpers ====================

fn test_operator_ids() -> [OperatorId; 4] {
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)]
}

/// The block root envelope tests bind envelopes to; `seed_decided_root` records this value.
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

/// Wraps a full envelope into the consensus value the store proposes for it.
fn envelope_consensus_value(
    pubkey: PublicKeyBytes,
    envelope: &ExecutionPayloadEnvelope<MainnetEthSpec>,
) -> EnvelopeConsensusData {
    let blinded = BlindedExecutionPayloadEnvelope::from_full(envelope);
    EnvelopeConsensusData {
        duty: ValidatorDuty {
            r#type: BEACON_ROLE_ENVELOPE_PROPOSER,
            pub_key: pubkey,
            slot: Slot::new(TEST_SLOT),
            validator_index: ValidatorIndex(STARTING_VALIDATOR_INDEX),
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        },
        version: DataVersion::from(ForkName::Gloas),
        data_ssz: VariableList::new(blinded.as_ssz_bytes())
            .expect("blinded envelope bytes should fit in the consensus data list"),
    }
}

/// A single-validator committee over `test_operator_ids()` and its validator's public key.
fn single_validator_committee() -> (CommitteeSetup, PublicKeyBytes) {
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
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

/// Records `test_decided_root()` for the validator at `TEST_SLOT` and returns it.
fn seed_decided_root(harness: &ValidatorStoreTestHarness, pubkey: PublicKeyBytes) -> Hash256 {
    let decided_root = test_decided_root();
    harness
        .validator_store
        .record_decided_block_context(
            pubkey,
            Slot::new(TEST_SLOT),
            DecidedBlockContext {
                beacon_block_root: decided_root,
                parent_block_root: Hash256::ZERO,
                execution_requests_root: Hash256::ZERO,
                builder_index: BUILDER_INDEX_SELF_BUILD,
                block_hash: ExecutionBlockHash::zero(),
                built_locally: true,
            },
        )
        .expect("seeding the decided context must succeed");
    decided_root
}

/// Builds a Gloas harness whose mock decides a mutated copy of the local envelope, with the
/// decided root already seeded. Returns the harness, the pubkey, and the local envelope.
fn forced_decision_harness(
    mutate_decided: impl FnOnce(&mut ExecutionPayloadEnvelope<MainnetEthSpec>),
) -> (
    ValidatorStoreTestHarness,
    PublicKeyBytes,
    ExecutionPayloadEnvelope<MainnetEthSpec>,
) {
    let (committee, pubkey) = single_validator_committee();
    let envelope = self_build_envelope(test_decided_root());
    let mut decided_envelope = envelope.clone();
    mutate_decided(&mut decided_envelope);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            decider: MockConsensusDecider::deciding_envelope(envelope_consensus_value(
                pubkey,
                &decided_envelope,
            )),
            ..gloas_options()
        },
    );
    seed_decided_root(&harness, pubkey);
    (harness, pubkey, envelope)
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

/// Asserts that the duty rejected before consensus: no decide call and no signature collection.
fn assert_rejected_before_consensus(harness: &ValidatorStoreTestHarness, context: &str) {
    assert!(
        harness.captured_decides.lock().is_empty(),
        "no consensus instance may start {context}"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen {context}"
    );
}

/// Drives one envelope decide call against the mock and unwraps the decided value.
async fn decide_envelope_seed(
    decider: &MockConsensusDecider,
    pubkey: PublicKeyBytes,
    seed: EnvelopeConsensusData,
) -> EnvelopeConsensusData {
    let result = ConsensusDecider::<MainnetEthSpec>::decide_instance(
        decider,
        EnvelopeProposerInstanceId {
            validator: pubkey,
            instance_height: (TEST_SLOT as usize).into(),
        },
        seed,
        Box::new(EnvelopeConsensusDataValidator::<MainnetEthSpec>::new(
            pubkey,
            ValidatorIndex(STARTING_VALIDATOR_INDEX),
            Slot::new(TEST_SLOT),
            Hash256::zero(),
        )),
        TimeoutMode::Relative {
            current_round_start_time: Instant::now(),
        },
        &IndexSet::from(test_operator_ids()),
    )
    .await
    .expect("the mock decider must not fail");
    match result {
        Completed::Success(decided) => decided,
        Completed::TimedOut => panic!("the mock decider must not time out"),
    }
}

// ==================== Mock and factory tests ====================

/// A forced envelope decision replaces the echo for envelope seeds.
#[tokio::test(flavor = "multi_thread")]
async fn mock_deciding_envelope_returns_the_forced_value() {
    let pubkey = PublicKeyBytes::empty();
    let seed_envelope = self_build_envelope(Hash256::from_low_u64_be(0xaaaa));
    let mut forced_envelope = seed_envelope.clone();
    forced_envelope.parent_beacon_block_root = Hash256::from_low_u64_be(0xbbbb);
    let seed = envelope_consensus_value(pubkey, &seed_envelope);
    let forced = envelope_consensus_value(pubkey, &forced_envelope);

    let decider = MockConsensusDecider::deciding_envelope(forced.clone());
    let decided = decide_envelope_seed(&decider, pubkey, seed).await;

    assert_eq!(
        decided, forced,
        "an envelope seed must decide as the forced value, not the echo"
    );
}

/// Without a forced decision, an envelope seed echoes back unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn mock_echoes_envelope_seed_without_a_forced_decision() {
    let pubkey = PublicKeyBytes::empty();
    let seed = envelope_consensus_value(
        pubkey,
        &self_build_envelope(Hash256::from_low_u64_be(0xaaaa)),
    );

    let decider = MockConsensusDecider::echoing();
    let decided = decide_envelope_seed(&decider, pubkey, seed.clone()).await;

    assert_eq!(
        decided, seed,
        "the echoing mock must return the seed unchanged"
    );
}

/// Every decide call lands in the capture, tagged with the seed type.
#[tokio::test(flavor = "multi_thread")]
async fn mock_captures_envelope_decide_calls() {
    let pubkey = PublicKeyBytes::empty();
    let seed = envelope_consensus_value(
        pubkey,
        &self_build_envelope(Hash256::from_low_u64_be(0xaaaa)),
    );

    let decider = MockConsensusDecider::echoing();
    let captured = decider.captured_decides();
    decide_envelope_seed(&decider, pubkey, seed).await;

    let calls = captured.lock();
    assert_eq!(calls.len(), 1, "one decide call must be captured");
    assert!(
        calls[0].data_type.contains("EnvelopeConsensusData"),
        "the captured call must be tagged with the envelope seed type"
    );
}

/// The factory passes pubkey, index, slot, and decided root into the value check.
#[tokio::test(flavor = "multi_thread")]
async fn factory_wires_duty_metadata_into_the_value_check() {
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(test_decided_root());
    let value = envelope_consensus_value(pubkey, &envelope);

    let validator = harness
        .validator_store
        .create_envelope_consensus_data_validator(
            pubkey,
            ValidatorIndex(STARTING_VALIDATOR_INDEX),
            Slot::new(TEST_SLOT),
            envelope.beacon_block_root,
        );

    assert!(
        validator.validate(&value, &value),
        "a matched envelope value must pass the factory-built value check"
    );

    let mut wrong_slot = value.clone();
    wrong_slot.duty.slot = Slot::new(TEST_SLOT + 1);
    assert!(
        !validator.validate(&wrong_slot, &value),
        "a value with the wrong slot must fail the factory-built value check"
    );
}

// ==================== Signing path tests ====================

/// A Gloas self-build envelope signs and returns the original message unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn gloas_self_build_envelope_signs_and_returns_the_original_message() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(seed_decided_root(&harness, pubkey));

    let signed = sign_envelope(&harness, pubkey, envelope.clone())
        .await
        .expect("a matched Gloas self-build envelope must sign successfully");

    assert_eq!(
        signed.message, envelope,
        "the returned message must be the input envelope unchanged"
    );
}

/// Exactly one post-consensus collection happens, over the blinded envelope root under
/// `Domain::BeaconBuilder`.
#[tokio::test(flavor = "multi_thread")]
async fn signing_root_is_the_blinded_envelope_root_under_beacon_builder_domain() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(seed_decided_root(&harness, pubkey));

    let spec = &harness.spec;
    let epoch = Slot::new(TEST_SLOT).epoch(MainnetEthSpec::slots_per_epoch());
    let domain_hash = spec.get_domain(
        epoch,
        Domain::BeaconBuilder,
        &spec.fork_at_epoch(epoch),
        harness.genesis_validators_root,
    );
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
        PartialSignatureKind::PostConsensus,
        "the partial signature kind must be post-consensus"
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
    let envelope = self_build_envelope(seed_decided_root(&harness, pubkey));

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

// ==================== Gate tests ====================

/// A pre-Gloas slot is rejected before consensus starts and before any signing.
#[tokio::test(flavor = "multi_thread")]
async fn pre_gloas_slot_is_rejected_before_consensus() {
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
    assert_rejected_before_consensus(&harness, "for a pre-Gloas envelope");
}

/// A non-self-build envelope is rejected before consensus starts and before any signing.
#[tokio::test(flavor = "multi_thread")]
async fn non_self_build_envelope_is_rejected_before_consensus() {
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
    assert_rejected_before_consensus(&harness, "for a non-self-build envelope");
}

/// A future-slot envelope is rejected before consensus starts and before any signing.
#[tokio::test(flavor = "multi_thread")]
async fn future_slot_envelope_is_rejected_before_consensus() {
    let (harness, pubkey) = gloas_harness();
    let mut envelope = self_build_envelope(test_decided_root());
    envelope.payload.slot_number = Slot::new(TEST_SLOT + 1);

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(result, Err(Error::GreaterThanCurrentSlot { .. })),
        "a future-slot envelope must be rejected with the dedicated error, got {result:?}"
    );
    assert_rejected_before_consensus(&harness, "for a future-slot envelope");
}

/// An envelope without a recorded decided block root is rejected before consensus.
#[tokio::test(flavor = "multi_thread")]
async fn missing_decided_root_is_rejected_before_consensus() {
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
        "an envelope without a decided root must be rejected with the dedicated error, got {result:?}"
    );
    assert_rejected_before_consensus(&harness, "without a decided root");
}

// ==================== Content gate tests ====================

/// A decided envelope that differs from the local one returns the sentinel error, and the
/// partial signature was still contributed before the gate.
#[tokio::test(flavor = "multi_thread")]
async fn decided_envelope_differing_from_local_returns_the_sentinel_after_signing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey, envelope) =
        forced_decision_harness(|decided| decided.payload.block_number = 1);

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeNotBuiltLocally { .. }
            ))
        ),
        "a decided envelope built elsewhere must return the sentinel error, got {result:?}"
    );
    assert_eq!(
        harness.captured_calls.lock().len(),
        1,
        "the partial signature must be contributed before the content gate"
    );
}

/// A decided envelope with the same payload root but different metadata still returns the
/// sentinel.
#[tokio::test(flavor = "multi_thread")]
async fn decided_envelope_with_same_payload_root_still_returns_the_sentinel() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey, envelope) = forced_decision_harness(|decided| {
        decided.parent_beacon_block_root = Hash256::from_low_u64_be(0x9999)
    });

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeNotBuiltLocally { .. }
            ))
        ),
        "the gate must compare the full blinded value, not just the payload root, got {result:?}"
    );
    assert_eq!(
        harness.captured_calls.lock().len(),
        1,
        "the partial signature must be contributed before the content gate"
    );
}

// ==================== Outcome metric tests ====================

/// A published outcome increments only the `published` label.
#[tokio::test(flavor = "multi_thread")]
async fn published_outcome_increments_only_the_published_label() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    let envelope = self_build_envelope(seed_decided_root(&harness, pubkey));
    let counters = OutcomeCounters::snapshot();

    sign_envelope(&harness, pubkey, envelope)
        .await
        .expect("the happy-path envelope duty must sign successfully");

    counters.assert_deltas(1, 0, 0);
}

/// The sentinel outcome increments `not_built_locally` and never `failed`.
#[tokio::test(flavor = "multi_thread")]
async fn sentinel_outcome_increments_not_built_locally_and_never_failed() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey, envelope) =
        forced_decision_harness(|decided| decided.payload.block_number = 1);
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        result.is_err(),
        "a decided envelope built elsewhere must not return a publishable envelope"
    );
    counters.assert_deltas(0, 1, 0);
}

/// A signature-collection failure increments the `failed` label.
#[tokio::test(flavor = "multi_thread")]
async fn collection_failure_increments_the_failed_label() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = harness_with_options(HarnessOptions {
        collector_failure: Some(CollectionError::EmptySignature),
        ..gloas_options()
    });
    let envelope = self_build_envelope(seed_decided_root(&harness, pubkey));
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

// ==================== Consensus failure tests ====================

/// A decided envelope that fails SSZ decoding counts as failed and signs nothing.
#[tokio::test(flavor = "multi_thread")]
async fn undecodable_decided_envelope_counts_as_failed_and_signs_nothing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (committee, pubkey) = single_validator_committee();
    let envelope = self_build_envelope(test_decided_root());
    let mut forced = envelope_consensus_value(pubkey, &envelope);
    forced.data_ssz = VariableList::new(vec![0xFF; 3]).expect("garbage bytes should fit");
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            decider: MockConsensusDecider::deciding_envelope(forced),
            ..gloas_options()
        },
    );
    seed_decided_root(&harness, pubkey);
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(SpecificError::InvalidQbftData(_)))
        ),
        "an undecodable decided envelope must surface as InvalidQbftData, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_eq!(
        harness.captured_decides.lock().len(),
        1,
        "exactly one decide call must reach consensus for an undecodable decided envelope"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen for an undecodable decided envelope"
    );
}

/// A consensus timeout counts as failed and signs nothing.
#[tokio::test(flavor = "multi_thread")]
async fn consensus_timeout_counts_as_failed_and_signs_nothing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = harness_with_options(HarnessOptions {
        decider: MockConsensusDecider::failing_envelope(ForcedEnvelopeFailure::Timeout),
        ..gloas_options()
    });
    let envelope = self_build_envelope(seed_decided_root(&harness, pubkey));
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(result, Err(Error::SpecificError(SpecificError::Timeout))),
        "a consensus timeout must surface as Timeout, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_eq!(
        harness.captured_decides.lock().len(),
        1,
        "exactly one decide call must reach consensus before a consensus timeout"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen after a consensus timeout"
    );
}

/// A consensus error counts as failed and signs nothing.
#[tokio::test(flavor = "multi_thread")]
async fn consensus_error_counts_as_failed_and_signs_nothing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = harness_with_options(HarnessOptions {
        decider: MockConsensusDecider::failing_envelope(ForcedEnvelopeFailure::Error(
            QbftError::QueueClosedError,
        )),
        ..gloas_options()
    });
    let envelope = self_build_envelope(seed_decided_root(&harness, pubkey));
    let counters = OutcomeCounters::snapshot();

    let result = sign_envelope(&harness, pubkey, envelope).await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(SpecificError::QbftError(
                QbftError::QueueClosedError
            )))
        ),
        "a consensus error must surface as the wrapped QbftError, got {result:?}"
    );
    counters.assert_deltas(0, 0, 1);
    assert_eq!(
        harness.captured_decides.lock().len(),
        1,
        "exactly one decide call must reach consensus before a consensus error"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen after a consensus error"
    );
}

// ==================== End-to-end tests ====================

/// End to end: `sign_block` records the decided root, then a matching envelope signs.
#[tokio::test(flavor = "multi_thread")]
async fn sign_block_then_matching_envelope_succeeds() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let (harness, pubkey) = gloas_harness();
    // `EmptyBlock::empty` fixes the slot to `spec.genesis_slot`, so the slot is set on the
    // inner struct before wrapping.
    let mut gloas_block = BeaconBlockGloas::<MainnetEthSpec>::empty(&harness.spec);
    gloas_block.slot = Slot::new(TEST_SLOT);
    let block = BeaconBlock::Gloas(gloas_block);
    let envelope = self_build_envelope(block.canonical_root());

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
        .expect("an envelope matching the block-recorded root must sign successfully");

    assert_eq!(
        signed.message, envelope,
        "the returned message must be the input envelope unchanged"
    );
}
