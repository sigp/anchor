//! Fork-aware tests for the committee attestation index, covering the `#1027`/`#1061` Gloas
//! change in `sign_committee_attestations` and `sign_committee_sync_committee_signatures`.
//!
//! The change makes the committee QBFT decide a single `attestation_data_index` at Gloas and
//! apply it to every validator's `attestation.data.index` before the signing root is computed,
//! so all operators sign identical roots cluster-wide. Pre-Gloas, the BN-supplied index is left
//! untouched.
//!
//! The mock decider (`common::MockConsensusDecider`) replaces real QBFT: it can echo the local
//! seed back, or force every `GloasBeaconVote` to decide a chosen index. That lets these tests
//! drive "decided index == local seed", "decided index != local seed", and "operators with
//! divergent local seeds converge on the same decided index" without a live consensus network.
use bls::FixedBytesExtended;
use futures::StreamExt;
use signature_collector::SignatureRequester;
use ssv_types::{
    OperatorId,
    consensus::{BeaconVote, GloasBeaconVote, QbftData},
};
use types::{
    Attestation, AttestationData, ChainSpec, Checkpoint, Domain, Epoch, EthSpec, Hash256,
    MainnetEthSpec, SignedRoot, Slot,
};
use validator_store::ValidatorStore;

use super::common::*;
use crate::Error;

type SignAttestationsResult = Vec<Result<Vec<(u64, Attestation<MainnetEthSpec>)>, Error>>;

const COMMITTEE_OPERATORS: [OperatorId; 4] =
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
const OUR_OPERATOR: OperatorId = OperatorId(1);
const COMMITTEE_VALIDATOR_COUNT: usize = 2;

/// The cluster-decided attestation index used across the Gloas tests. Differs from the seed
/// (`SEED_INDEX`) so the tests prove the decided value (not the local seed) reaches the signature.
const DECIDED_INDEX: u64 = 1;
/// The local seed index each operator proposes before consensus.
const SEED_INDEX: u64 = 0;

/// Recomputes the attester signing root the same way the production path does, so assertions can
/// pin the exact signing root without re-deriving the domain logic by hand. `block_root`,
/// `source`, and `target` are the all-zero values the harness seeds; `index` is the value under
/// test.
fn expected_attester_signing_root(
    spec: &ChainSpec,
    genesis_validators_root: Hash256,
    slot: Slot,
    index: u64,
) -> Hash256 {
    let epoch = slot.epoch(MainnetEthSpec::slots_per_epoch());
    let domain = spec.get_domain(
        epoch,
        Domain::BeaconAttester,
        &spec.fork_at_epoch(epoch),
        genesis_validators_root,
    );
    let data = AttestationData {
        slot,
        index,
        beacon_block_root: Hash256::zero(),
        source: Checkpoint::default(),
        target: Checkpoint::default(),
    };
    data.signing_root(domain)
}

/// Drives `sign_attestations` to completion and unwraps every committee batch.
async fn run_sign_attestations(
    harness: &ValidatorStoreTestHarness,
    attestations: Vec<validator_store::AttestationToSign<MainnetEthSpec>>,
) -> Vec<(u64, Attestation<MainnetEthSpec>)> {
    let results: SignAttestationsResult = harness
        .validator_store
        .sign_attestations(attestations)
        .collect()
        .await;
    results
        .into_iter()
        .flat_map(|batch| batch.expect("committee batch should succeed"))
        .collect()
}

// ==================== Gloas: decided index applied ====================

/// At Gloas, the single cluster-decided `attestation_data_index` must be written onto every
/// validator's `attestation.data.index` BEFORE the signing root is computed, even when it differs
/// from this operator's local seed. This is the core `#1027` apply step (C3/C6): without it, an
/// operator would sign over its stale local index and diverge from the cluster.
#[tokio::test(flavor = "multi_thread")]
async fn gloas_decided_index_applied_to_every_validator_attestation() {
    // Arrange: Gloas slot, local seed index 0, but the cluster decides index 1.
    let committee = create_committee_setup(&COMMITTEE_OPERATORS, COMMITTEE_VALIDATOR_COUNT, 0);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OUR_OPERATOR,
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            forced_gloas_index: Some(DECIDED_INDEX),
            ..Default::default()
        },
    );
    harness.seed_gloas_voting_context(SEED_INDEX);
    let attestations = vec![
        harness.create_attestation(0, 0),
        harness.create_attestation(0, 1),
    ];

    // Act
    let signed = run_sign_attestations(&harness, attestations).await;

    // Assert: every produced attestation carries the decided index, not the seed.
    assert_eq!(
        signed.len(),
        COMMITTEE_VALIDATOR_COUNT,
        "expected one signed attestation per validator"
    );
    for (_, attestation) in &signed {
        assert_eq!(
            attestation.data().index,
            DECIDED_INDEX,
            "Gloas must apply the cluster-decided index to data.index, not the local seed"
        );
    }

    // Assert: each captured signing root is the root over the decided index.
    let expected_root = expected_attester_signing_root(
        &harness.spec,
        harness.genesis_validators_root,
        Slot::new(TEST_SLOT),
        DECIDED_INDEX,
    );
    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        COMMITTEE_VALIDATOR_COUNT,
        "expected one sign_and_collect call per validator"
    );
    for call in captured.iter() {
        assert_eq!(
            call.signing_root, expected_root,
            "each validator must sign the attester root computed over the decided index"
        );
    }
}

// ==================== Gloas: cluster-wide convergence ====================

/// Two operators whose LOCAL seeds carry divergent attestation indices must still produce
/// identical signing roots once they decide the SAME index. This models the cluster-wide
/// agreement guarantee of `#1027` (C6): the signed root is a function of the decided index, never
/// the per-operator seed. Modeled with two independent harnesses (one per operator) since the
/// mock decider does not run a shared instance.
#[tokio::test(flavor = "multi_thread")]
async fn gloas_all_operators_converge_on_decided_index_signing_root() {
    // Arrange: operator A seeds index 0, operator B seeds index 1; both decide index 1.
    let operator_a_seed = 0;
    let operator_b_seed = 1;

    let committee_a = create_committee_setup(&COMMITTEE_OPERATORS, COMMITTEE_VALIDATOR_COUNT, 0);
    let harness_a = ValidatorStoreTestHarness::new_with_options(
        vec![committee_a],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            forced_gloas_index: Some(DECIDED_INDEX),
            ..Default::default()
        },
    );
    harness_a.seed_gloas_voting_context(operator_a_seed);

    let committee_b = create_committee_setup(&COMMITTEE_OPERATORS, COMMITTEE_VALIDATOR_COUNT, 0);
    let harness_b = ValidatorStoreTestHarness::new_with_options(
        vec![committee_b],
        OperatorId(2),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            forced_gloas_index: Some(DECIDED_INDEX),
            ..Default::default()
        },
    );
    harness_b.seed_gloas_voting_context(operator_b_seed);

    // Precondition: the two operators genuinely start from divergent local seeds.
    assert_ne!(
        operator_a_seed, operator_b_seed,
        "test is only meaningful if local seeds diverge"
    );

    // Act
    let signed_a =
        run_sign_attestations(&harness_a, vec![harness_a.create_attestation(0, 0)]).await;
    let signed_b =
        run_sign_attestations(&harness_b, vec![harness_b.create_attestation(0, 0)]).await;

    // Assert: both operators sign over the decided index, producing identical roots.
    let expected_root = expected_attester_signing_root(
        &harness_a.spec,
        harness_a.genesis_validators_root,
        Slot::new(TEST_SLOT),
        DECIDED_INDEX,
    );
    let root_a = harness_a.captured_calls.lock()[0].signing_root;
    let root_b = harness_b.captured_calls.lock()[0].signing_root;

    assert_eq!(
        root_a, root_b,
        "operators with divergent local seeds must converge on one signing root"
    );
    assert_eq!(
        root_a, expected_root,
        "the converged root must be the root computed over the decided index"
    );
    assert_eq!(
        signed_a[0].1.data().index,
        DECIDED_INDEX,
        "operator A must sign the decided index"
    );
    assert_eq!(
        signed_b[0].1.data().index,
        DECIDED_INDEX,
        "operator B must sign the decided index"
    );
}

// ==================== Pre-Gloas: index untouched ====================

/// At Electra (post-Electra, pre-Gloas) the decider has no `decided_index` (`None`), so the apply
/// step is skipped and `data.index` stays at its BN-supplied value, which is `0` at Electra+.
/// This guards the `#1027` C5 boundary: the index-apply must NOT fire before Gloas.
#[tokio::test(flavor = "multi_thread")]
async fn pre_gloas_electra_attestation_index_is_zero_and_untouched() {
    // Arrange: Electra-at-genesis spec (so TEST_SLOT is Electra, not Gloas), echo decider.
    let committee = create_committee_setup(&COMMITTEE_OPERATORS, COMMITTEE_VALIDATOR_COUNT, 0);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OUR_OPERATOR,
        HarnessOptions {
            spec: electra_at_genesis_spec(),
            ..Default::default()
        },
    );
    // Pre-Gloas path uses the Base vote; the duty's data.index starts at 0.
    harness.seed_voting_context();
    let attestations = vec![
        harness.create_attestation(0, 0),
        harness.create_attestation(0, 1),
    ];

    // Act
    let signed = run_sign_attestations(&harness, attestations).await;

    // Assert: index stays 0 (Electra+ BN-supplied value), never overwritten.
    assert_eq!(signed.len(), COMMITTEE_VALIDATOR_COUNT);
    for (_, attestation) in &signed {
        assert_eq!(
            attestation.data().index,
            0,
            "pre-Gloas Electra attestation index must remain 0 and untouched"
        );
    }

    // Assert: signing roots match the root over index 0.
    let expected_root = expected_attester_signing_root(
        &harness.spec,
        harness.genesis_validators_root,
        Slot::new(TEST_SLOT),
        0,
    );
    for call in harness.captured_calls.lock().iter() {
        assert_eq!(
            call.signing_root, expected_root,
            "pre-Gloas signing root must be computed over the untouched index 0"
        );
    }
}

/// Pre-Electra, the BN supplies `data.index == committee_index`, and the pre-Gloas path must
/// leave it untouched (the guarded apply is skipped because `decided_index` is `None`). This
/// covers the `#1027` C4 invariant.
///
/// `ChainSpec::mainnet()` places Electra far in the future, so the default harness spec already
/// positions `TEST_SLOT` at a pre-Electra fork: no special spec is needed. The duty is seeded
/// with a non-zero `data.index` so a regression that wrongly overwrites it (e.g. to 0 or to a
/// decided value) would surface here.
#[tokio::test(flavor = "multi_thread")]
async fn pre_gloas_pre_electra_attestation_index_equals_committee_index() {
    // Arrange: default mainnet spec => TEST_SLOT is pre-Electra; echo decider.
    let committee_index: u64 = 3;
    let voting_block_root = Hash256::repeat_byte(0xBA);
    let voting_source = Checkpoint {
        epoch: Epoch::new(0),
        root: Hash256::repeat_byte(0x51),
    };
    let voting_target = Checkpoint {
        epoch: Epoch::new(0),
        root: Hash256::repeat_byte(0x71),
    };
    let voting_vote = BeaconVote {
        block_root: voting_block_root,
        source: voting_source,
        target: voting_target,
    };
    let expected_base_hash = voting_vote.hash();
    let committee = create_committee_setup(&COMMITTEE_OPERATORS, COMMITTEE_VALIDATOR_COUNT, 0);
    let harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR);
    harness.seed_base_voting_context_with_vote(voting_vote);
    // Seed each validator's duty with data.index == committee_index, as a pre-Electra BN would.
    let attestations = vec![
        harness.create_attestation_with_index(0, 0, committee_index),
        harness.create_attestation_with_index(0, 1, committee_index),
    ];
    for attestation in &attestations {
        let duty_data = attestation.attestation.data();
        assert_ne!(duty_data.beacon_block_root, voting_block_root);
        assert_ne!(duty_data.source, voting_source);
        assert_ne!(duty_data.target, voting_target);
    }

    // Act
    let signed = run_sign_attestations(&harness, attestations).await;

    // Assert: the BN-supplied committee index survives unchanged.
    assert_eq!(signed.len(), COMMITTEE_VALIDATOR_COUNT);
    for (_, attestation) in &signed {
        assert_eq!(
            attestation.data().index,
            committee_index,
            "pre-Electra attestation index must equal the BN committee index, untouched"
        );
        assert_eq!(attestation.data().beacon_block_root, voting_block_root);
        assert_eq!(attestation.data().source, voting_source);
        assert_eq!(attestation.data().target, voting_target);
    }

    // Assert: consensus and signing use the shared voting-context vote, while retaining the
    // pre-Electra duty index.
    let slot = Slot::new(TEST_SLOT);
    let epoch = slot.epoch(MainnetEthSpec::slots_per_epoch());
    let domain = harness.spec.get_domain(
        epoch,
        Domain::BeaconAttester,
        &harness.spec.fork_at_epoch(epoch),
        harness.genesis_validators_root,
    );
    let expected_root = AttestationData {
        slot,
        index: committee_index,
        beacon_block_root: voting_block_root,
        source: voting_source,
        target: voting_target,
    }
    .signing_root(domain);
    for call in harness.captured_calls.lock().iter() {
        assert_eq!(
            call.signing_root, expected_root,
            "pre-Electra signing root must use the voting-context fields and untouched index"
        );
        let SignatureRequester::Committee { base_hash, .. } = &call.requester else {
            panic!("attestation signing must use the committee signature requester");
        };
        assert_eq!(*base_hash, expected_base_hash);
    }
}

// ==================== Gloas: attestation and sync paths share the seed ====================

/// `#1061` (L2/L3/L4): for the same `(committee, slot)` at Gloas, the attestation path and the
/// sync-committee path must seed the committee QBFT from the SAME `voting_context.vote`. With the
/// mock decider both decide the same value, so the decided hash (the partial-signature base) and
/// the `CommitteeInstanceId` must match across the two paths. Because the mock does not model a
/// shared live instance, this asserts seed/hash/instance-id equality rather than a literal
/// instance count.
#[tokio::test(flavor = "multi_thread")]
async fn sync_and_attestation_paths_seed_identical_gloas_instance() {
    // Arrange: one Gloas committee, both an attestation duty and a sync duty for the same slot.
    let voting_block_root = Hash256::repeat_byte(0xB1);
    let voting_source = Checkpoint {
        epoch: Epoch::new(0),
        root: Hash256::repeat_byte(0x52),
    };
    let voting_target = Checkpoint {
        epoch: Epoch::new(0),
        root: Hash256::repeat_byte(0x72),
    };
    let voting_vote = GloasBeaconVote {
        block_root: voting_block_root,
        source: voting_source,
        target: voting_target,
        attestation_data_index: SEED_INDEX,
    };
    let committee = create_committee_setup(&COMMITTEE_OPERATORS, COMMITTEE_VALIDATOR_COUNT, 0);
    let committee_id = committee.cluster.committee_id();
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OUR_OPERATOR,
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            forced_gloas_index: Some(DECIDED_INDEX),
            ..Default::default()
        },
    );
    harness.seed_gloas_voting_context_with_vote(voting_vote);

    // The decided hash both paths feed into the signature collector is the hash of the decided
    // GloasBeaconVote. Both paths build that vote from the same seed, so this is the shared base.
    let expected_base_hash = GloasBeaconVote {
        block_root: voting_block_root,
        source: voting_source,
        target: voting_target,
        attestation_data_index: DECIDED_INDEX,
    }
    .hash();

    // Act: drive the attestation path, capture its committee base hash + committee id.
    let duty = harness.create_attestation(0, 0);
    assert_ne!(duty.attestation.data().beacon_block_root, voting_block_root);
    assert_ne!(duty.attestation.data().source, voting_source);
    assert_ne!(duty.attestation.data().target, voting_target);
    let attestation_signed = run_sign_attestations(&harness, vec![duty]).await;
    let (attestation_base_hash, attestation_committee_id) =
        committee_call_base_hash_and_id(&harness);

    // Assert: the attestation path applies all decided voting-context fields, not the incoming
    // duty's fields. The mock changes only the index, preserving the remaining seed fields.
    assert_eq!(attestation_signed.len(), 1);
    let signed_data = attestation_signed[0].1.data();
    assert_eq!(signed_data.beacon_block_root, voting_block_root);
    assert_eq!(signed_data.source, voting_source);
    assert_eq!(signed_data.target, voting_target);
    assert_eq!(signed_data.index, DECIDED_INDEX);

    // Reset captures so the sync path's calls are isolated.
    harness.captured_calls.lock().clear();

    // Act: drive the sync path over the same committee/slot.
    let sync_results: Vec<_> = harness
        .validator_store
        .sign_sync_committee_signatures(vec![harness.create_sync_message(0, 0)])
        .collect()
        .await;
    for result in &sync_results {
        result
            .as_ref()
            .expect("sync committee batch should succeed");
    }
    let (sync_base_hash, sync_committee_id) = committee_call_base_hash_and_id(&harness);

    // Assert: both paths feed the same decided hash as the partial-signature base.
    assert_eq!(
        attestation_base_hash, expected_base_hash,
        "attestation path must base partial signatures on the decided GloasBeaconVote hash"
    );
    assert_eq!(
        sync_base_hash, expected_base_hash,
        "sync path must base partial signatures on the same decided GloasBeaconVote hash"
    );
    assert_eq!(
        attestation_base_hash, sync_base_hash,
        "both paths share one decided hash for the same (committee, slot) at Gloas"
    );

    // Assert: both target the same committee id (the CommitteeInstanceId committee component).
    assert_eq!(attestation_committee_id, committee_id);
    assert_eq!(sync_committee_id, committee_id);
    assert_eq!(
        attestation_committee_id, sync_committee_id,
        "both paths address the same committee instance"
    );
}

/// Reads the single committee `sign_and_collect` call captured so far, returning its
/// partial-signature base hash and committee id. Panics if there is not exactly one committee
/// call, so a test that captured nothing fails loudly instead of silently passing.
fn committee_call_base_hash_and_id(
    harness: &ValidatorStoreTestHarness,
) -> (Hash256, ssv_types::CommitteeId) {
    let captured = harness.captured_calls.lock();
    let committee_calls: Vec<_> = captured
        .iter()
        .filter_map(|call| match &call.requester {
            SignatureRequester::Committee { base_hash, .. } => {
                Some((*base_hash, call.metadata.committee_id))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        committee_calls.len(),
        1,
        "expected exactly one committee sign_and_collect call, got {}",
        committee_calls.len()
    );
    committee_calls[0]
}
