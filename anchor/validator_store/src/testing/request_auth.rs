//! Integration tests for the RequestAuth path in `sign_request_auth_v1()`.
//!
//! These tests pin the decisions unique to this duty relative to its kind-8 sibling
//! (`sign_proposer_preferences`):
//!  - the signing domain is the *fixed* application domain from builder-specs #165
//!    (`DOMAIN_REQUEST_AUTH` = 0x0B000001 combined with the genesis fork version and a zeroed
//!    genesis_validators_root). It is fork-epoch invariant, unlike the epoch-keyed
//!    `Domain::ProposerPreferences`,
//!  - the collection bound is slot-aware; see `request_auth_timeout_is_slot_aware` for the
//!    three-case rule,
//!  - the proposal slot is tree-hashed into the signing root itself (`slot` is a field of
//!    `RequestAuth`), so a given root always carries the same slot.
//!
//! Collector independence between kind 8 and kind 9 is deliberately not tested here: the
//! signature collector keys collections by `(signing_root, validator_index)` in
//! `signature_collector`'s `get_or_spawn`, and the two duties' domain-separated roots can never
//! collide, so independence follows structurally. Admission-budget independence on the receive
//! side is covered by message_validator's #1281 tests.
//!
//! Scope of the slot assertions: as in the ProposerPreferences module, these tests assert
//! `call.metadata.slot`, the value captured by the mock at the `sign_and_collect` trait boundary.
//! `create_message` copies that value verbatim into the on-wire `PartialSignatureMessages.slot`
//! (see `signature_collector::SignatureCollectorManager::create_message`), and that verbatim copy
//! is covered by signature_collector's own tests.
use std::{sync::LazyLock, task::Poll, time::Duration};

use bls::Signature;
use builder_types::{RequestAuth, RequestAuthData};
use signature_collector::{CollectionError, SignatureRequester};
use ssv_types::{OperatorId, msgid::Role, partial_sig::PartialSignatureKind};
use tree_hash::TreeHash;
use types::{ChainSpec, Domain, Epoch, EthSpec, Hash256, MainnetEthSpec, SignedRoot, Slot};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{
    Error, REQUEST_AUTH_COLLECTION_TIMEOUT_SLOTS, REQUEST_AUTH_PROPOSAL_SLOT_TIMEOUT, SpecificError,
};

/// Non-zero so a correctly targeted validator is distinguishable from an accidental default 0.
const STARTING_VALIDATOR_INDEX: usize = 5;
/// Number of epochs a lookahead proposal slot sits ahead of the send slot. Two epochs places the
/// Gloas fork boundary strictly between the send epoch and the proposal epoch in the success
/// test, which is what makes its epoch-keyed-domain guard falsifiable.
const LOOKAHEAD_EPOCHS: u64 = 2;
/// Builder auth data used by every fixture. The bytes follow the `RequestAuth::data` convention
/// (the builder's advertised URL) but the store treats them as opaque, so any non-empty value
/// exercises the same path. The known-answer vector below uses the same bytes, so its constants
/// double as an independent check of this fixture's merkleization.
const TEST_AUTH_DATA: &[u8] = b"https://builder.example/";
/// One paused-clock tick used to bracket a timeout deadline: a poll at `bound - epsilon` must be
/// pending and a poll at `bound + epsilon` must be resolved.
const TIMER_EPSILON: Duration = Duration::from_millis(1);

/// Serializes the metric-reading tests against each other. Both the classification test and the
/// slot-aware timeout test increment labels of the global prometheus
/// `REQUEST_AUTH_RECONSTRUCTION_FAILURES` counter (every timeout failure lands in the
/// `insufficient_partial_signatures` bucket), so concurrent execution would make the
/// classification test's delta assertions racy. A tokio mutex rather than std because the guard
/// is held across awaits.
static METRIC_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Builds a `RequestAuth` fixture. The store signs whatever it is handed, so fixed auth data is
/// sufficient; `proposal_slot` is a parameter because it is tree-hashed into the signing root and
/// selects the slot-aware collection bound the tests assert on.
fn create_request_auth(proposal_slot: Slot) -> RequestAuth {
    RequestAuth {
        data: RequestAuthData::new(TEST_AUTH_DATA.to_vec())
            .expect("auth data fits the ByteList limit"),
        slot: proposal_slot,
    }
}

// ==================== Success / signing-root tests ====================

/// `sign_request_auth_v1` collects a single-validator signature and echoes the input back in the
/// resulting `SignedRequestAuth`, committing to the `RequestAuth` under the *fixed* request-auth
/// application domain (builder-specs #165), with the collection pinned to kind
/// `RequestAuth`, role `ProposerPreferences`, a `SingleValidator` requester, and an envelope slot
/// equal to `request_auth.slot`.
///
/// The harness runs on a spec with Gloas activated exactly at `LOOKAHEAD_EPOCHS` and the proposal
/// slot sits `LOOKAHEAD_EPOCHS` ahead of the send slot, so a fork boundary lies strictly between
/// the send epoch (genesis fork version) and the proposal epoch (Gloas fork version). The fixed
/// domain must be entirely unaffected by that boundary. Falsifiability guard: the observed root is
/// also asserted NOT to equal a recompute under the epoch-keyed `Domain::ProposerPreferences` at
/// the proposal epoch. A copy-paste regression to the kind-8 domain call would flip that
/// assertion; the fork boundary guarantees the two computations differ in the fork-version bytes
/// as well as the domain type, so even a partially copied regression is observable.
///
/// Slashing-protection tripwire (folded in from the sibling module's standalone test): the
/// harness runs with slashing protection *enabled* and the validator registered, and the final
/// assertion block exports the interchange to prove the path recorded neither a block proposal
/// nor an attestation. A regression that routed this duty through the slashing DB would either
/// fail the signing call or leave a record behind, flipping one of these assertions.
#[tokio::test(flavor = "multi_thread")]
async fn request_auth_success_pins_root_kind_role_mode_and_envelope_slot() {
    // Arrange
    let our_operator_id = OperatorId(1);
    let committee =
        create_committee_setup(&PRIMARY_COMMITTEE_OPERATOR_IDS, 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    // Gloas at epoch LOOKAHEAD_EPOCHS puts a fork boundary strictly inside the lookahead window;
    // the fixed request-auth domain must not care, while the epoch-keyed kind-8 domain does.
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            spec: gloas_at_epoch_spec(Epoch::new(LOOKAHEAD_EPOCHS)),
            disable_slashing_protection: false,
            ..Default::default()
        },
    );
    let future_proposal_slot =
        Slot::new(TEST_SLOT + MainnetEthSpec::slots_per_epoch() * LOOKAHEAD_EPOCHS);
    let request_auth = create_request_auth(future_proposal_slot);

    // Act
    let result = harness
        .validator_store
        .sign_request_auth_v1(pubkey, request_auth.clone())
        .await;

    // Assert
    let signed = result.expect("request auth signing should succeed");
    assert_eq!(
        signed.message, request_auth,
        "signed message should echo the input request auth unchanged"
    );
    assert_eq!(
        signed.signature,
        Signature::infinity().expect("infinity signature"),
        "the signature should be the one the mock collector produced"
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
        PartialSignatureKind::RequestAuth,
        "partial signature messages should be tagged with the RequestAuth kind"
    );
    assert_eq!(
        call.metadata.role,
        Role::ProposerPreferences,
        "the network message should be routed under the ProposerPreferences role"
    );
    assert_eq!(
        call.metadata.slot, request_auth.slot,
        "the collected partial-signature slot should equal the request auth's proposal slot"
    );

    // The signing root must use the fixed application domain: genesis fork version and zeroed
    // genesis_validators_root regardless of the proposal epoch's fork.
    let expected_root = request_auth.signing_root(harness.spec.get_request_auth_domain());
    assert_eq!(
        call.signing_root, expected_root,
        "signing root should commit to the request auth under the fixed DOMAIN_REQUEST_AUTH \
         application domain"
    );
    // Falsifiability guard: recompute under the kind-8 epoch-keyed domain at the proposal epoch.
    // The Gloas boundary inside the lookahead window makes that domain differ from the fixed one
    // in both the domain type and the fork-version bytes, so a copy-paste regression to the
    // ProposerPreferences domain call would flip this assertion.
    let proposal_epoch = request_auth.slot.epoch(MainnetEthSpec::slots_per_epoch());
    let epoch_keyed_domain = harness.spec.get_domain(
        proposal_epoch,
        Domain::ProposerPreferences,
        &harness.spec.fork_at_epoch(proposal_epoch),
        harness.genesis_validators_root,
    );
    assert_ne!(
        call.signing_root,
        request_auth.signing_root(epoch_keyed_domain),
        "signing root must NOT be the epoch-keyed ProposerPreferences domain at the proposal \
         epoch: the request-auth domain is a fixed application domain, invariant across fork \
         boundaries"
    );
    drop(captured);

    // Slashing tripwire: the call succeeded with slashing protection enabled, and the DB (which
    // registered exactly this validator during harness setup) recorded nothing. The length check
    // keeps the emptiness assertion non-vacuous.
    let interchange = harness
        .slashing_protection
        .export_all_interchange_info(harness.genesis_validators_root)
        .expect("interchange export should succeed");
    assert_eq!(
        interchange.data.len(),
        1,
        "the harness should have registered exactly one validator in the slashing DB"
    );
    assert!(
        interchange
            .data
            .iter()
            .all(|record| record.signed_blocks.is_empty() && record.signed_attestations.is_empty()),
        "signing a request auth must not record anything in the slashing DB"
    );
}

// ==================== Timeout tests ====================

/// The collection bound is slot-aware (`request_auth_collection_bound`), table-driven over the
/// three positions of the proposal slot relative to the clock:
///  - future slot (cache-warming): the bound is `REQUEST_AUTH_COLLECTION_TIMEOUT_SLOTS` slots; a
///    no-quorum collection is still pending just before that bound and resolves to
///    `CollectionTimeout` just after,
///  - current slot (block-production): the bound is the fail-fast
///    `REQUEST_AUTH_PROPOSAL_SLOT_TIMEOUT` (1s), bracketed the same way,
///  - past slot (steady state: LH's preferences loop revisits elapsed proposer slots every tick):
///    the bound resolves to a decline, which fails on the first poll before the collection future
///    is ever constructed. The mock records ZERO collection calls (so no partial signature would
///    have been broadcast) and neither reconstruction-failure label moves: declines bypass the
///    failure reporter so structural noise cannot pollute the divergence metric.
///
/// All three failures surface as `SignatureCollectionFailed(CollectionTimeout)`. The mock
/// collector hangs forever, so only the production bound can resolve the bounded cases; the
/// bracketing polls under a paused clock make each case's exact bound falsifiable in both
/// directions (a longer bound fails the "resolved just after" poll, a shorter one fails the
/// "still pending just before" poll).
///
/// Joins `METRIC_TEST_LOCK` because the bounded cases increment the
/// `insufficient_partial_signatures` label the classification test asserts deltas on, and the
/// decline case reads both labels for its own zero-delta assertions.
#[tokio::test(start_paused = true)]
async fn request_auth_timeout_is_slot_aware() {
    let _guard = METRIC_TEST_LOCK.lock().await;

    struct TimeoutCase {
        name: &'static str,
        proposal_slot: Slot,
        /// The expected collection bound; `Duration::ZERO` marks the decline case, which fails
        /// on the first poll with no collector call and no failure-metric increment.
        expected_bound: Duration,
    }

    // The harness spec is mainnet, so the future-slot bound is slot duration times the
    // production constant; recomputing it from the same inputs keeps the case table free of
    // magic seconds.
    let mainnet_slot_duration = ChainSpec::mainnet().get_slot_duration();
    let cases = [
        TimeoutCase {
            name: "future proposal slot",
            proposal_slot: Slot::new(TEST_SLOT + 2),
            expected_bound: mainnet_slot_duration * REQUEST_AUTH_COLLECTION_TIMEOUT_SLOTS,
        },
        TimeoutCase {
            name: "current proposal slot",
            proposal_slot: Slot::new(TEST_SLOT),
            expected_bound: REQUEST_AUTH_PROPOSAL_SLOT_TIMEOUT,
        },
        TimeoutCase {
            name: "past proposal slot",
            proposal_slot: Slot::new(TEST_SLOT - 1),
            expected_bound: Duration::ZERO,
        },
    ];

    let our_operator_id = OperatorId(1);
    for case in cases {
        // Arrange: a fresh harness per case so captured-call counts do not leak across cases. The
        // collector captures each call and then never resolves, so only the production bound can
        // unblock the bounded cases.
        let committee =
            create_committee_setup(&PRIMARY_COMMITTEE_OPERATOR_IDS, 1, STARTING_VALIDATOR_INDEX);
        let pubkey = committee.validators[0].public_key;
        let harness = ValidatorStoreTestHarness::new_with_options(
            vec![committee],
            our_operator_id,
            HarnessOptions {
                collector_hangs: true,
                ..Default::default()
            },
        );
        let request_auth = create_request_auth(case.proposal_slot);

        // Act + Assert: drive the future by hand under the paused clock so the deadline can be
        // bracketed on both sides.
        let fut = harness
            .validator_store
            .sign_request_auth_v1(pubkey, request_auth);
        tokio::pin!(fut);

        let result = if case.expected_bound.is_zero() {
            // A decline must stay out of the failure reporter: elapsed proposal slots recur
            // every tick in steady state, so routing them through the reporter would pollute
            // the divergence metric with structural noise. The label deltas are read around
            // this case only; the reads are race-free because this test holds
            // `METRIC_TEST_LOCK` (the earlier bounded cases DO increment the insufficient
            // label, but those increments land before these before-values are captured).
            let metric = crate::metrics::REQUEST_AUTH_RECONSTRUCTION_FAILURES
                .as_ref()
                .expect("metric should be created");
            let insufficient_counter = metric.with_label_values(&[
                crate::metrics::REQUEST_AUTH_FAILURE_INSUFFICIENT_PARTIAL_SIGNATURES,
            ]);
            let infra_counter =
                metric.with_label_values(&[crate::metrics::REQUEST_AUTH_FAILURE_INFRA]);
            let insufficient_before = insufficient_counter.get();
            let infra_before = infra_counter.get();

            // The decline must fail on the very first poll: it happens before the collection
            // future is ever constructed.
            let Poll::Ready(result) = futures::poll!(fut.as_mut()) else {
                panic!(
                    "{}: a past-slot decline must fail on the first poll",
                    case.name
                );
            };
            assert!(
                harness.captured_calls.lock().is_empty(),
                "{}: a past-slot request must never reach the collector (no partial signature \
                 broadcast)",
                case.name
            );
            assert_eq!(
                insufficient_counter.get() - insufficient_before,
                0,
                "{}: a decline must not increment the insufficient_partial_signatures \
                 reconstruction-failure label",
                case.name
            );
            assert_eq!(
                infra_counter.get() - infra_before,
                0,
                "{}: a decline must not increment the infra reconstruction-failure label",
                case.name
            );
            result
        } else {
            // First poll starts the collection (captured by the mock) and arms the bound.
            assert!(
                futures::poll!(fut.as_mut()).is_pending(),
                "{}: the collection should be in flight after the first poll",
                case.name
            );
            tokio::time::advance(case.expected_bound - TIMER_EPSILON).await;
            assert!(
                futures::poll!(fut.as_mut()).is_pending(),
                "{}: the call must still be pending just before the bound elapses",
                case.name
            );
            tokio::time::advance(TIMER_EPSILON * 2).await;
            let Poll::Ready(result) = futures::poll!(fut.as_mut()) else {
                panic!(
                    "{}: the call must have resolved just after the bound elapsed",
                    case.name
                );
            };
            assert_eq!(
                harness.captured_calls.lock().len(),
                1,
                "{}: exactly one sign_and_collect call should have been captured before the \
                 collector hung",
                case.name
            );
            result
        };
        assert!(
            matches!(
                result,
                Err(Error::SpecificError(
                    SpecificError::SignatureCollectionFailed(CollectionError::CollectionTimeout)
                ))
            ),
            "{}: expected CollectionTimeout from the slot-aware bound, got: {result:?}",
            case.name
        );
    }
}

// ==================== Failure classification / metrics tests ====================

/// Collection failures are classified into the two labels of
/// `REQUEST_AUTH_RECONSTRUCTION_FAILURES` by `report_request_auth_collection_failure`:
///  - a no-quorum timeout (the hanging collector plus the current-slot bound, resolved by the
///    production `collect_within` deadline) lands in `insufficient_partial_signatures`,
///  - an `EmptySignature` collection error (the same infra injection the sibling module uses) lands
///    in `infra`.
///
/// Each phase asserts the cross-label zero delta too, pinning the classification boundary: a
/// timeout drifting into `infra` would hide divergence signals, and an infra failure drifting
/// into `insufficient_partial_signatures` would silently inflate the divergence estimate. The
/// metric lives in the process-global prometheus registry, so deltas are only reliable under
/// `METRIC_TEST_LOCK`.
#[tokio::test(start_paused = true)]
async fn request_auth_failure_classification_increments_metrics() {
    let _guard = METRIC_TEST_LOCK.lock().await;

    // Arrange
    let metric = crate::metrics::REQUEST_AUTH_RECONSTRUCTION_FAILURES
        .as_ref()
        .expect("metric should be created");
    let insufficient_counter = metric
        .with_label_values(&[crate::metrics::REQUEST_AUTH_FAILURE_INSUFFICIENT_PARTIAL_SIGNATURES]);
    let infra_counter = metric.with_label_values(&[crate::metrics::REQUEST_AUTH_FAILURE_INFRA]);
    let insufficient_before = insufficient_counter.get();
    let infra_before = infra_counter.get();

    let our_operator_id = OperatorId(1);

    // Act (phase 1): a no-quorum timeout. The collector hangs and the current-slot proposal
    // selects the 1s fail-fast bound, which the paused clock auto-advances past, so the failure
    // is the genuine `CollectionTimeout` from `collect_within`.
    let committee =
        create_committee_setup(&PRIMARY_COMMITTEE_OPERATOR_IDS, 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            collector_hangs: true,
            ..Default::default()
        },
    );
    let result = harness
        .validator_store
        .sign_request_auth_v1(pubkey, create_request_auth(Slot::new(TEST_SLOT)))
        .await;

    // Assert (phase 1)
    assert!(
        matches!(
            result,
            Err(Error::SpecificError(
                SpecificError::SignatureCollectionFailed(CollectionError::CollectionTimeout)
            ))
        ),
        "expected the no-quorum timeout surfaced as SignatureCollectionFailed, got: {result:?}"
    );
    assert_eq!(
        insufficient_counter.get() - insufficient_before,
        1,
        "CollectionTimeout should increment the insufficient_partial_signatures \
         reconstruction-failure metric once"
    );
    assert_eq!(
        infra_counter.get() - infra_before,
        0,
        "CollectionTimeout must not leak into the infra reconstruction-failure metric"
    );

    // Act (phase 2): an infra failure. EmptySignature classifies as the infra failure class.
    let committee =
        create_committee_setup(&PRIMARY_COMMITTEE_OPERATOR_IDS, 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        our_operator_id,
        HarnessOptions {
            collector_failure: Some(CollectionError::EmptySignature),
            ..Default::default()
        },
    );
    let result = harness
        .validator_store
        .sign_request_auth_v1(pubkey, create_request_auth(Slot::new(TEST_SLOT)))
        .await;

    // Assert (phase 2)
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
    assert_eq!(
        insufficient_counter.get() - insufficient_before,
        1,
        "the infra failure must not leak into the insufficient_partial_signatures metric (its \
         delta stays at phase 1's single increment)"
    );
}

// ==================== Known-answer vector ====================

/// Known-answer test for the request-auth signing root, independent of the harness and of
/// Lighthouse's own hashing helpers on the assertion side.
///
/// The expected values were derived OUTSIDE the Lighthouse code path with a hand-rolled SSZ
/// merkleization script (Python, hashlib only): the `data` ByteList[4096] root is
/// `mix_in_length(merkleize_to_depth7(pad32(data)), 24)`; the `slot` root is the uint64
/// little-endian bytes zero-padded to 32; the container root is `sha256(data_root || slot_root)`;
/// the domain is `0x0B000001` (DOMAIN_REQUEST_AUTH, builder-specs #165) followed by the first 28
/// bytes of `fork_data_root(genesis_fork_version=0x00000000, genesis_validators_root=0)`; and the
/// signing root is `sha256(object_root || domain)`. To re-derive, run:
///
/// ```python
/// import hashlib
/// def H(a,b): return hashlib.sha256(a+b).digest()
/// Z=b'\x00'*32
/// data=b"https://builder.example/"
/// zh=[Z]
/// for i in range(10): zh.append(H(zh[-1],zh[-1]))
/// node=data.ljust(32,b'\x00')
/// for d in range(7): node=H(node, zh[d])
/// data_root=H(node, len(data).to_bytes(32,'little'))
/// slot_root=(1234567).to_bytes(8,'little').ljust(32,b'\x00')
/// object_root=H(data_root, slot_root)
/// fork_data_root=H(bytes(4).ljust(32,b'\x00'), Z)
/// domain=bytes([0x0B,0,0,1])+fork_data_root[:28]
/// signing_root=H(object_root, domain)
/// ```
///
/// Any change to the domain constant, the `RequestAuth` field set or ordering, or the ByteList
/// limit (and hence merkleization depth) flips at least one of these constants.
#[test]
fn request_auth_signing_root_known_answer_vector() {
    // Arrange
    let spec = ChainSpec::mainnet();
    let request_auth = create_request_auth(Slot::new(1_234_567));
    let expected_domain: Hash256 =
        "0b000001f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a9"
            .parse()
            .expect("valid hash literal");
    let expected_tree_hash_root: Hash256 =
        "95be6fadb620639ec806c3e3a0e040a7e5c554316b8a118b532c29c401a084f3"
            .parse()
            .expect("valid hash literal");
    let expected_signing_root: Hash256 =
        "c2affcb4cb84affb3580f31a9edab5ab234073917a8ac4c703dcb3983392aa50"
            .parse()
            .expect("valid hash literal");

    // Act
    let domain = spec.get_request_auth_domain();

    // Assert
    assert_eq!(
        domain, expected_domain,
        "DOMAIN_REQUEST_AUTH must be 0x0B000001 over the genesis fork version and a zeroed \
         genesis_validators_root"
    );
    assert_eq!(
        request_auth.tree_hash_root(),
        expected_tree_hash_root,
        "RequestAuth tree hash root must match the independently derived SSZ merkleization"
    );
    assert_eq!(
        request_auth.signing_root(domain),
        expected_signing_root,
        "RequestAuth signing root must match the independently derived value"
    );
}
