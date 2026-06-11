//! Integration tests for the PTC payload attestation path in `sign_payload_attestation()`.
use std::sync::LazyLock;

use bls::FixedBytesExtended;
use signature_collector::{CollectionError, SignatureRequester};
use ssv_types::{OperatorId, msgid::Role, partial_sig::PartialSignatureKind};
use types::{
    ChainSpec, Domain, EthSpec, Hash256, MainnetEthSpec, PayloadAttestationData, SignedRoot, Slot,
};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{Error, SpecificError};

/// Non-zero so a correctly echoed beacon index is distinguishable from an accidental default 0.
const STARTING_VALIDATOR_INDEX: usize = 5;

/// Serializes the two metric tests against each other. Both read the same labels of the global
/// prometheus `PTC_RECONSTRUCTION_FAILURES` counter, so concurrent execution would make their
/// cross-label delta assertions racy. A tokio mutex rather than std because the guard is held
/// across awaits on a multi-thread runtime.
static METRIC_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn test_operator_ids() -> [OperatorId; 4] {
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)]
}

/// The store signs whatever data it is handed (LH already fetched it at the slot cutoff), so a
/// fixed fixture is sufficient; no voting context seeding is needed.
fn create_payload_attestation_data() -> PayloadAttestationData {
    PayloadAttestationData {
        beacon_block_root: Hash256::zero(),
        slot: Slot::new(TEST_SLOT),
        payload_present: true,
        blob_data_available: true,
    }
}

/// `sign_payload_attestation` resolves the validator's beacon index, collects a signature in
/// single-validator mode, and echoes the input data back in the resulting message.
#[tokio::test(flavor = "multi_thread")]
async fn sign_payload_attestation_builds_message_with_validator_index() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);
    let data = create_payload_attestation_data();

    // Act
    let result = harness
        .validator_store
        .sign_payload_attestation(pubkey, data.clone())
        .await;

    // Assert
    let message = result.expect("payload attestation signing should succeed");
    assert_eq!(
        message.validator_index, STARTING_VALIDATOR_INDEX as u64,
        "message should carry the validator's beacon index"
    );
    assert_eq!(message.data, data, "message should echo the input data");

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
        PartialSignatureKind::PTCAttester,
        "partial signature messages should be tagged with the PTC attester kind"
    );
    assert_eq!(
        call.metadata.role,
        Role::PTCAttester,
        "the network message should be routed under the PTC attester role"
    );
    assert_eq!(
        call.metadata.slot, data.slot,
        "collection should be scheduled for the attestation slot"
    );

    // Recompute the root independently (with the same mainnet spec and zero
    // genesis_validators_root the harness uses) to lock the PTC-specific signing decisions that
    // no other test covers: the `Domain::PTCAttester` choice, the epoch derived from the
    // attestation slot, and the signed object being the `PayloadAttestationData` itself. A
    // regression to e.g. `Domain::BeaconAttester` fails here.
    let spec = ChainSpec::mainnet();
    let epoch = data.slot.epoch(MainnetEthSpec::slots_per_epoch());
    let domain = spec.get_domain(
        epoch,
        Domain::PTCAttester,
        &spec.fork_at_epoch(epoch),
        Hash256::zero(),
    );
    let expected_root = data.signing_root(domain);
    assert_eq!(
        call.signing_root, expected_root,
        "signing root should commit to the payload attestation data under the PTC domain"
    );
}

/// A validator without a beacon index fails fast with `MissingIndex` and never starts a partial
/// signature round, because the index is resolved before collection.
#[tokio::test(flavor = "multi_thread")]
async fn sign_payload_attestation_missing_index_errors() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let mut committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    // The validators table stores the index in a nullable column and the in-memory state keeps
    // metadata as passed, so a metadata gap flows through harness construction unchanged.
    committee.validators[0].index = None;
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);
    let data = create_payload_attestation_data();

    // Act
    let result = harness
        .validator_store
        .sign_payload_attestation(pubkey, data)
        .await;

    // Assert
    assert!(
        matches!(
            result,
            Err(Error::SpecificError(SpecificError::MissingIndex))
        ),
        "expected MissingIndex error, got: {result:?}"
    );
    assert!(
        harness.captured_calls.lock().is_empty(),
        "no signature round should start for a validator without an index"
    );
}

/// A collection failure surfaces as `SignatureCollectionFailed` and increments the
/// `no_signature` reconstruction-failure metric exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn sign_payload_attestation_collection_failure_increments_no_signature_metric() {
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
            collector_failure: Some(CollectionError::QueueClosedError),
            disable_slashing_protection: true,
        },
    );
    let data = create_payload_attestation_data();
    // The metric lives in the global prometheus registry shared by every test in the process,
    // so we assert on the delta. The infra metric test also touches this label (asserting a
    // zero delta), so the delta is only reliable because both tests hold `METRIC_TEST_LOCK`;
    // future failure tests must join that serialization or use distinct labels.
    let no_signature_counter = crate::metrics::PTC_RECONSTRUCTION_FAILURES
        .as_ref()
        .expect("metric should be created")
        .with_label_values(&[crate::metrics::PTC_FAILURE_NO_SIGNATURE]);
    let count_before = no_signature_counter.get();

    // Act
    let result = harness
        .validator_store
        .sign_payload_attestation(pubkey, data)
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
        no_signature_counter.get() - count_before,
        1,
        "QueueClosedError should increment the no_signature reconstruction-failure metric once"
    );
}

/// An infrastructure failure surfaces as `SignatureCollectionFailed` and increments the `infra`
/// reconstruction-failure metric, leaving the `no_signature` metric untouched.
#[tokio::test(flavor = "multi_thread")]
async fn sign_payload_attestation_infra_failure_increments_infra_metric() {
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
            collector_failure: Some(CollectionError::EmptySignature),
            disable_slashing_protection: true,
        },
    );
    let data = create_payload_attestation_data();
    // The metric lives in the global prometheus registry shared by every test in the process,
    // so we assert on deltas. The `no_signature` zero-delta read races with the QueueClosedError
    // test's increment, so the deltas are only reliable because both tests hold
    // `METRIC_TEST_LOCK`; future failure tests must join that serialization or use distinct
    // labels.
    let metric = crate::metrics::PTC_RECONSTRUCTION_FAILURES
        .as_ref()
        .expect("metric should be created");
    let infra_counter = metric.with_label_values(&[crate::metrics::PTC_FAILURE_INFRA]);
    let no_signature_counter =
        metric.with_label_values(&[crate::metrics::PTC_FAILURE_NO_SIGNATURE]);
    let infra_before = infra_counter.get();
    let no_signature_before = no_signature_counter.get();

    // Act
    let result = harness
        .validator_store
        .sign_payload_attestation(pubkey, data)
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
    // The zero delta pins the classification boundary: `no_signature` is the SIP-94
    // observation-divergence upper bound, so an infra variant drifting into that bucket would
    // silently inflate the divergence estimate.
    assert_eq!(
        no_signature_counter.get() - no_signature_before,
        0,
        "infra failures must not leak into the no_signature divergence metric"
    );
}

/// `sign_payload_attestation` succeeds with slashing protection enabled, proving the path never
/// consults the slashing DB.
///
/// Tripwire mechanism: the harness slashing DB is created empty and no validator is ever
/// registered in it, so any slashing-protection check would fail for an unregistered validator.
/// If such a check were ever added to this code path, this call would flip from Ok to Err, which
/// makes the success assertion a real behavioral assertion rather than a tautology.
#[tokio::test(flavor = "multi_thread")]
async fn sign_payload_attestation_does_not_touch_slashing_db() {
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
        },
    );
    let data = create_payload_attestation_data();

    // Act
    let result = harness
        .validator_store
        .sign_payload_attestation(pubkey, data.clone())
        .await;

    // Assert
    let message = result.expect("signing should succeed despite slashing protection being enabled");
    assert_eq!(
        message.validator_index, STARTING_VALIDATOR_INDEX as u64,
        "message should carry the validator's beacon index"
    );
    assert_eq!(message.data, data, "message should echo the input data");
}
