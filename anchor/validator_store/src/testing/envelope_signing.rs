//! Envelope-signing duty tests (SIP-94).

use std::sync::LazyLock;

use bls::{FixedBytesExtended, PublicKeyBytes};
use eth2::types::FullBlockContents;
use qbft::Completed;
use qbft_manager::{ConsensusDecider, EnvelopeProposerInstanceId, TimeoutMode};
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
    BeaconBlock, BeaconBlockGloas, Domain, EmptyBlock, EthSpec, ExecutionPayloadEnvelope,
    ExecutionPayloadGloas, ExecutionRequestsGloas, ForkName, Hash256, MainnetEthSpec, SignedRoot,
    Slot, consts::gloas::BUILDER_INDEX_SELF_BUILD,
};
use validator_store::{UnsignedBlock, ValidatorStore};

use super::common::*;
use crate::{Error, SpecificError};

/// Validator index the single-validator committee starts at.
const STARTING_VALIDATOR_INDEX: usize = 5;

/// Serializes every test that records an envelope outcome: the labels live in the global
/// prometheus registry, so concurrent recordings would race the delta assertions. Tokio mutex
/// because the guard is held across awaits.
static METRIC_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn test_operator_ids() -> [OperatorId; 4] {
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)]
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

/// The `(published, not_built_locally, failed)` outcome counters the metric tests assert
/// deltas on; each test takes its own before-reads under `METRIC_TEST_LOCK`.
fn envelope_outcome_counters() -> (
    crate::metrics::IntCounter,
    crate::metrics::IntCounter,
    crate::metrics::IntCounter,
) {
    let metric = crate::metrics::ENVELOPE_SIGNING_OUTCOMES
        .as_ref()
        .expect("metric should be created");
    (
        metric.with_label_values(&[crate::metrics::ENVELOPE_OUTCOME_PUBLISHED]),
        metric.with_label_values(&[crate::metrics::ENVELOPE_OUTCOME_NOT_BUILT_LOCALLY]),
        metric.with_label_values(&[crate::metrics::ENVELOPE_OUTCOME_FAILED]),
    )
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
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let envelope = self_build_envelope(Hash256::from_low_u64_be(0xdec1));
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

/// A Gloas self-build envelope signs and returns the original message unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn gloas_self_build_envelope_signs_and_returns_the_original_message() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let decided_root = Hash256::from_low_u64_be(0xdec1);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(TEST_SLOT), decided_root)
        .expect("seeding the decided root must succeed");
    let envelope = self_build_envelope(decided_root);

    let signed = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope.clone())
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
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let decided_root = Hash256::from_low_u64_be(0xdec1);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(TEST_SLOT), decided_root)
        .expect("seeding the decided root must succeed");
    let envelope = self_build_envelope(decided_root);

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

    harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
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

/// A pre-Gloas slot is rejected before consensus starts and before any signing.
#[tokio::test(flavor = "multi_thread")]
async fn pre_gloas_slot_is_rejected_before_consensus() {
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: electra_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let envelope = self_build_envelope(Hash256::from_low_u64_be(0xdec1));

    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::EnvelopeBeforeGloas { .. }
            ))
        ),
        "a pre-Gloas envelope must be rejected with the dedicated error, got {result:?}"
    );
    assert!(
        harness.captured_decides.lock().is_empty(),
        "no consensus instance may start for a pre-Gloas envelope"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen for a pre-Gloas envelope"
    );
}

/// A non-self-build envelope is rejected before consensus starts and before any signing.
#[tokio::test(flavor = "multi_thread")]
async fn non_self_build_envelope_is_rejected_before_consensus() {
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let mut envelope = self_build_envelope(Hash256::from_low_u64_be(0xdec1));
    envelope.builder_index = 7;

    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(SpecificError::EnvelopeNotSelfBuild {
                builder_index: 7
            }))
        ),
        "a non-self-build envelope must be rejected with the dedicated error, got {result:?}"
    );
    assert!(
        harness.captured_decides.lock().is_empty(),
        "no consensus instance may start for a non-self-build envelope"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen for a non-self-build envelope"
    );
}

/// An envelope without a recorded decided block root is rejected before consensus.
#[tokio::test(flavor = "multi_thread")]
async fn missing_decided_root_is_rejected_before_consensus() {
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let envelope = self_build_envelope(Hash256::from_low_u64_be(0xdec1));

    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::DecidedRootUnavailable { .. }
            ))
        ),
        "an envelope without a decided root must be rejected with the dedicated error, got {result:?}"
    );
    assert!(
        harness.captured_decides.lock().is_empty(),
        "no consensus instance may start without a decided root"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature collection may happen without a decided root"
    );
}

/// A decided envelope that differs from the local one returns the sentinel error, and the
/// partial signature was still contributed before the gate.
#[tokio::test(flavor = "multi_thread")]
async fn decided_envelope_differing_from_local_returns_the_sentinel_after_signing() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let decided_root = Hash256::from_low_u64_be(0xdec1);
    let envelope = self_build_envelope(decided_root);
    let mut other_envelope = envelope.clone();
    other_envelope.payload.block_number = 1;
    let forced = envelope_consensus_value(pubkey, &other_envelope);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            forced_envelope_decision: Some(forced),
            ..Default::default()
        },
    );
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(TEST_SLOT), decided_root)
        .expect("seeding the decided root must succeed");

    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

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
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let decided_root = Hash256::from_low_u64_be(0xdec1);
    let envelope = self_build_envelope(decided_root);
    let mut other_envelope = envelope.clone();
    other_envelope.parent_beacon_block_root = Hash256::from_low_u64_be(0x9999);
    let forced = envelope_consensus_value(pubkey, &other_envelope);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            forced_envelope_decision: Some(forced),
            ..Default::default()
        },
    );
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(TEST_SLOT), decided_root)
        .expect("seeding the decided root must succeed");

    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

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

/// The envelope path succeeds with slashing protection enabled and writes no block-proposal
/// record at the duty slot: a first-time probe insertion afterwards is still accepted as safe.
#[tokio::test(flavor = "multi_thread")]
async fn envelope_signing_does_not_touch_the_slashing_db() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: false,
            ..Default::default()
        },
    );
    let decided_root = Hash256::from_low_u64_be(0xdec1);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(TEST_SLOT), decided_root)
        .expect("seeding the decided root must succeed");
    let envelope = self_build_envelope(decided_root);

    let signed = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope.clone())
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

/// A published outcome increments only the `published` label.
#[tokio::test(flavor = "multi_thread")]
async fn published_outcome_increments_only_the_published_label() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let decided_root = Hash256::from_low_u64_be(0xdec1);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(TEST_SLOT), decided_root)
        .expect("seeding the decided root must succeed");
    let envelope = self_build_envelope(decided_root);
    let (published, not_built_locally, failed) = envelope_outcome_counters();
    let published_before = published.get();
    let not_built_locally_before = not_built_locally.get();
    let failed_before = failed.get();

    harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await
        .expect("the happy-path envelope duty must sign successfully");

    assert_eq!(
        published.get() - published_before,
        1,
        "a published envelope must increment the published label once"
    );
    assert_eq!(
        not_built_locally.get() - not_built_locally_before,
        0,
        "a published envelope must not touch the not_built_locally label"
    );
    assert_eq!(
        failed.get() - failed_before,
        0,
        "a published envelope must not touch the failed label"
    );
}

/// The sentinel outcome increments `not_built_locally` and never `failed`.
#[tokio::test(flavor = "multi_thread")]
async fn sentinel_outcome_increments_not_built_locally_and_never_failed() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let decided_root = Hash256::from_low_u64_be(0xdec1);
    let envelope = self_build_envelope(decided_root);
    let mut other_envelope = envelope.clone();
    other_envelope.payload.block_number = 1;
    let forced = envelope_consensus_value(pubkey, &other_envelope);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            forced_envelope_decision: Some(forced),
            ..Default::default()
        },
    );
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(TEST_SLOT), decided_root)
        .expect("seeding the decided root must succeed");
    let (_, not_built_locally, failed) = envelope_outcome_counters();
    let not_built_locally_before = not_built_locally.get();
    let failed_before = failed.get();

    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

    assert!(
        result.is_err(),
        "a decided envelope built elsewhere must not return a publishable envelope"
    );
    assert_eq!(
        not_built_locally.get() - not_built_locally_before,
        1,
        "the sentinel outcome must increment the not_built_locally label once"
    );
    assert_eq!(
        failed.get() - failed_before,
        0,
        "the sentinel outcome must never count as a failure"
    );
}

/// A signature-collection failure increments the `failed` label.
#[tokio::test(flavor = "multi_thread")]
async fn collection_failure_increments_the_failed_label() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            collector_failure: Some(CollectionError::EmptySignature),
            ..Default::default()
        },
    );
    let decided_root = Hash256::from_low_u64_be(0xdec1);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(TEST_SLOT), decided_root)
        .expect("seeding the decided root must succeed");
    let envelope = self_build_envelope(decided_root);
    let (published, not_built_locally, failed) = envelope_outcome_counters();
    let published_before = published.get();
    let not_built_locally_before = not_built_locally.get();
    let failed_before = failed.get();

    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::SignatureCollectionFailed(CollectionError::EmptySignature)
            ))
        ),
        "a collection failure must surface as SignatureCollectionFailed, got {result:?}"
    );
    assert_eq!(
        failed.get() - failed_before,
        1,
        "a collection failure must increment the failed label once"
    );
    assert_eq!(
        published.get() - published_before,
        0,
        "a collection failure must not touch the published label"
    );
    assert_eq!(
        not_built_locally.get() - not_built_locally_before,
        0,
        "a collection failure must not touch the not_built_locally label"
    );
}

/// End to end: `sign_block` records the decided root, then a matching envelope signs.
#[tokio::test(flavor = "multi_thread")]
async fn sign_block_then_matching_envelope_succeeds() {
    let _guard = METRIC_TEST_LOCK.lock().await;
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
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
    let signed = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope.clone())
        .await
        .expect("an envelope matching the block-recorded root must sign successfully");

    assert_eq!(
        signed.message, envelope,
        "the returned message must be the input envelope unchanged"
    );
}
