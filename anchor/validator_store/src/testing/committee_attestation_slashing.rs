//! Slashing-protection tests for the committee attestation path.
//!
//! The default harness disables slashing protection (under `CheckSlashability::No` nothing is
//! recorded), so these tests enable it. With protection enabled the harness registers every
//! validator in the slashing DB, `slashing_protection_attestations` records every signed
//! candidate, and only slash-safe (and publishable) attestations reach the returned batch.
use futures::StreamExt;
use signature_collector::SignatureRequester;
use slashing_protection::{CheckSlashability, NotSafe, Safe};
use ssv_types::{OperatorId, consensus::BeaconVote};
use types::{AttestationData, Domain, EthSpec, Hash256, MainnetEthSpec, SingleAttestation, Slot};
use validator_store::ValidatorStore;

use super::common::*;

const COMMITTEE_OPERATORS: [OperatorId; 4] =
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
const OUR_OPERATOR: OperatorId = OperatorId(1);
const COMMITTEE_VALIDATOR_COUNT: usize = 2;

/// Builds a single-committee harness with slashing protection ENABLED (validators registered).
fn slashing_enabled_harness() -> ValidatorStoreTestHarness {
    let committee = create_committee_setup(&COMMITTEE_OPERATORS, COMMITTEE_VALIDATOR_COUNT, 0);
    ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OUR_OPERATOR,
        HarnessOptions {
            disable_slashing_protection: false,
            ..Default::default()
        },
    )
}

/// Drives `sign_attestations` for both committee validators and unwraps the single batch.
async fn sign_both_validators(harness: &ValidatorStoreTestHarness) -> Vec<SingleAttestation> {
    let attestations = vec![
        harness.create_attestation(0, 0),
        harness.create_attestation(0, 1),
    ];
    run_sign_attestations(harness, attestations).await
}

/// Builds a `BeaconVote` at target/source epoch 0 with the given block root, so two votes with
/// distinct roots are a double vote (same target epoch, different data) once both are signed.
fn vote_with_block_root(block_root: Hash256) -> BeaconVote {
    BeaconVote {
        block_root,
        source: ValidatorStoreTestHarness::zero_checkpoint(),
        target: ValidatorStoreTestHarness::zero_checkpoint(),
    }
}

/// Recomputes the attester domain the production path uses for `TEST_SLOT` (target epoch 0).
fn attester_domain(harness: &ValidatorStoreTestHarness) -> Hash256 {
    let epoch = Slot::new(TEST_SLOT).epoch(MainnetEthSpec::slots_per_epoch());
    harness.spec.get_domain(
        epoch,
        Domain::BeaconAttester,
        &harness.spec.fork_at_epoch(epoch),
        harness.genesis_validators_root,
    )
}

// ==================== Slashing protection through the migrated path ====================

/// First-time attestations pass slashing protection and are all returned for publication.
#[tokio::test(flavor = "multi_thread")]
async fn slashing_protection_returns_first_time_attestations() {
    // Arrange
    let harness = slashing_enabled_harness();
    harness.seed_base_voting_context_with_vote(vote_with_block_root(Hash256::repeat_byte(0xAA)));

    // Act
    let signed = sign_both_validators(&harness).await;

    // Assert
    assert_eq!(
        signed.len(),
        COMMITTEE_VALIDATOR_COUNT,
        "first-time attestations must all pass slashing protection"
    );
}

/// Re-signing the exact same attestation data is detected as `SameData` and skipped: the second
/// batch succeeds but publishes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn slashing_protection_skips_same_data_resign() {
    // Arrange
    let harness = slashing_enabled_harness();
    harness.seed_base_voting_context_with_vote(vote_with_block_root(Hash256::repeat_byte(0xAA)));
    let first = sign_both_validators(&harness).await;
    assert_eq!(first.len(), COMMITTEE_VALIDATOR_COUNT);

    // Act: sign the same duties over the unchanged voting context.
    let second = sign_both_validators(&harness).await;

    // Assert: the batch succeeds (asserted inside the helper) but every entry is skipped.
    assert!(
        second.is_empty(),
        "previously signed attestation data must be skipped as SameData, got {} entries",
        second.len()
    );
}

/// A conflicting attestation (same target epoch, different data) is blocked by slashing
/// protection: signing still runs, but nothing is returned for publication.
#[tokio::test(flavor = "multi_thread")]
async fn slashing_protection_blocks_conflicting_attestation() {
    // Arrange: sign over one vote first.
    let harness = slashing_enabled_harness();
    harness.seed_base_voting_context_with_vote(vote_with_block_root(Hash256::repeat_byte(0xAA)));
    let first = sign_both_validators(&harness).await;
    assert_eq!(first.len(), COMMITTEE_VALIDATOR_COUNT);

    // Act: re-seed a vote with the same target epoch but a different block root (a double vote)
    // and sign again.
    harness.seed_base_voting_context_with_vote(vote_with_block_root(Hash256::repeat_byte(0xBB)));
    let second = sign_both_validators(&harness).await;

    // Assert: the conflicting batch is fully withheld.
    assert!(
        second.is_empty(),
        "conflicting attestations must be blocked as slashable, got {} entries",
        second.len()
    );
    // The filter is applied AFTER signature collection: both rounds collected one signature per
    // validator, so the second round's signing calls still happened.
    assert_eq!(
        harness.captured_calls.lock().len(),
        2 * COMMITTEE_VALIDATOR_COUNT,
        "slashing protection must filter at publication, not before signature collection"
    );
}

// ==================== Identity mismatch: publishable flag ====================

/// A duty whose `attester_index` does not match our own metadata still signs (dropping it before
/// collection would stall the committee's exact-count partial-signature batch) and still reaches
/// the slashing DB, but is withheld from the returned publication batch. The other validators'
/// attestations are unaffected.
#[tokio::test(flavor = "multi_thread")]
async fn identity_mismatch_recorded_in_slashing_db_but_withheld_from_publication() {
    // Arrange: validator 0 keeps its correct identity, validator 1's duty carries a wrong
    // attester_index.
    let harness = slashing_enabled_harness();
    harness.seed_voting_context();
    let good = harness.create_attestation(0, 0);
    let good_attester_index = good.attester_index;
    let mut bad = harness.create_attestation(0, 1);
    bad.attester_index += 100;
    let bad_pubkey = bad.pubkey;

    // Act
    let results: SignAttestationsResult = harness
        .validator_store
        .sign_attestations(vec![good, bad])
        .collect()
        .await;

    // Assert: only the well-formed duty is returned for publication.
    assert_eq!(results.len(), 1, "expected one stream item per committee");
    let signed: Vec<SingleAttestation> = results
        .into_iter()
        .flat_map(|batch| batch.expect("committee batch should succeed"))
        .collect();
    assert_eq!(
        signed.len(),
        1,
        "the mismatched duty must be absent from the publication batch"
    );
    assert_eq!(
        signed[0].attester_index, good_attester_index,
        "the well-formed duty must be published normally"
    );

    // Assert: the signature collector still sent for BOTH validators with the full committee
    // batch size, so the mismatch caused no committee stall.
    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        COMMITTEE_VALIDATOR_COUNT,
        "the mismatched duty must still go through signature collection"
    );
    for call in captured.iter() {
        let SignatureRequester::Committee {
            validator_partial_signature_batch_size,
            ..
        } = &call.requester
        else {
            panic!(
                "expected SignatureRequester::Committee, got: {:?}",
                call.requester
            );
        };
        assert_eq!(
            *validator_partial_signature_batch_size, COMMITTEE_VALIDATOR_COUNT,
            "the batch size must still count every attesting validator in the committee"
        );
    }
    drop(captured);

    // Assert: the mismatched validator's signed data WAS recorded in the slashing DB. The exact
    // data the production path signed is the seeded zero vote at TEST_SLOT (pre-Electra path
    // leaves data.index at 0), which equals the duty data `create_attestation` builds; probing it
    // again yields `SameData`, which only an existing identical record can produce.
    let signed_data = harness.create_attestation(0, 1).data;
    let domain = attester_domain(&harness);
    let same_data_probe = harness
        .slashing_protection
        .check_and_insert_attestations(&[(
            &signed_data,
            &bad_pubkey,
            domain,
            CheckSlashability::Yes,
        )])
        .expect("slashing DB transaction should succeed");
    assert!(
        matches!(same_data_probe[0], Ok(Safe::SameData)),
        "the mismatched validator's data must already be recorded, got {:?}",
        same_data_probe[0]
    );

    // And a conflicting attestation for the same validator is now refused as a double vote.
    let conflicting_data = AttestationData {
        beacon_block_root: Hash256::repeat_byte(0xCC),
        ..signed_data
    };
    let conflict_probe = harness
        .slashing_protection
        .check_and_insert_attestations(&[(
            &conflicting_data,
            &bad_pubkey,
            domain,
            CheckSlashability::Yes,
        )])
        .expect("slashing DB transaction should succeed");
    assert!(
        matches!(conflict_probe[0], Err(NotSafe::InvalidAttestation(_))),
        "a conflicting attestation for the mismatched validator must be refused, got {:?}",
        conflict_probe[0]
    );
}
