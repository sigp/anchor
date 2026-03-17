//! Integration tests for the committee-batching attestation path in `sign_attestations()`.
//!
//! `sign_attestations()` groups attestations by `CommitteeId`, processes each committee
//! concurrently via `FuturesUnordered`, and returns a `Stream` of results. These tests verify
//! the grouping, streaming, per-committee consensus flow, and failure isolation.

use std::{collections::HashSet, time::Duration};

use bls::FixedBytesExtended;
use futures::StreamExt;
use ssv_types::OperatorId;
use ssz::Decode;
use types::{Attestation, MainnetEthSpec};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{Error, SpecificError};

type SignAttestationsResult = Vec<Result<Vec<(u64, Attestation<MainnetEthSpec>)>, Error>>;

// ==================== Test 1: Stream grouping with single-operator timeout ====================

/// Test 1: Multiple SSV committees in one `sign_attestations()` call produce separate stream
/// batches (one per committee).
///
/// This test verifies the grouping logic in `sign_attestations()`: attestations belonging to
/// different `CommitteeId` values are processed independently, and the returned `Stream` yields
/// one `Result` item per committee. Because we have a single QBFT manager (our operator) without
/// other operators routing messages back, QBFT consensus will time out. The key assertion is
/// that the stream produces exactly as many items as there are distinct committees, confirming
/// the `FuturesUnordered` fan-out logic.
///
/// The QBFT timeout path returns `Completed::TimedOut`, which `sign_committee_attestations`
/// maps to `Err(Timeout)`. However, `sign_attestations` catches that error and returns
/// `Ok(Vec::new())` per committee. So we expect 2 stream items, both `Ok(empty)`.
#[tokio::test(flavor = "multi_thread")]
async fn test_sign_attestations_produces_one_stream_item_per_committee() {
    // Arrange
    let rsa_private_key = generate_rsa_keypair();
    let rsa_pubkey = rsa_public_from_private(&rsa_private_key);

    let our_operator_id = OperatorId(1);

    // Create 2 committees with distinct operator sets to guarantee different `CommitteeId`s.
    // Committee A uses operators [1, 2, 3, 4], Committee B uses operators [1, 5, 6, 7].
    let operator_ids_a: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
    let operator_ids_b: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)];

    let committee_a = create_committee_setup(
        &operator_ids_a,
        2, // 2 validators in committee A
        our_operator_id,
        &rsa_pubkey,
        0, // starting_validator_index
    );
    let committee_b = create_committee_setup(
        &operator_ids_b,
        1, // 1 validator in committee B
        our_operator_id,
        &rsa_pubkey,
        100, // starting_validator_index (offset to avoid collision)
    );

    // Verify the committees have distinct IDs
    let committee_id_a = committee_a.cluster.committee_id();
    let committee_id_b = committee_b.cluster.committee_id();
    assert_ne!(
        committee_id_a, committee_id_b,
        "Test requires two distinct committee IDs"
    );

    let (executor, _signal) = create_test_executor();

    let harness = ValidatorStoreTestHarness::new(
        vec![committee_a, committee_b],
        our_operator_id,
        rsa_private_key,
        executor,
    );

    harness.seed_voting_context();

    // Advance clock far past all QBFT timeouts so single-operator consensus times out instantly
    harness.set_clock_for_instant_timeout();

    // Build attestations: 2 from committee A, 1 from committee B
    let attestations = vec![
        harness.create_attestation_to_sign(0, 0),
        harness.create_attestation_to_sign(0, 1),
        harness.create_attestation_to_sign(1, 0),
    ];

    // Act: call `sign_attestations` and collect stream items
    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    // Collect results with a timeout to avoid hanging if consensus never completes.
    // Each committee runs QBFT `decide_instance`, which will time out since we only have
    // one operator (no quorum). The timeout duration is per-slot (slot_duration / 3 = 4s),
    // plus QBFT round timeouts. We give 60s total for both committees to time out.
    let mut results: SignAttestationsResult = Vec::new();
    let collect_timeout = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(item) = stream.next().await {
            results.push(item);
        }
    })
    .await;

    // Assert
    // The stream should have completed (not timed out at our 60s boundary).
    if collect_timeout.is_err() {
        panic!(
            "Stream collection timed out after 60s. \
             Got {} items before timeout. Expected 2 items (one per committee).",
            results.len()
        );
    }

    assert_eq!(
        results.len(),
        2,
        "sign_attestations should produce exactly one stream item per committee. \
         Got {} items instead of 2.",
        results.len()
    );

    // Both items should be Ok (errors are caught and converted to Ok(Vec::new()) inside
    // sign_attestations). The actual attestations will be empty because QBFT consensus
    // times out without enough operators to form a quorum.
    for (i, result) in results.iter().enumerate() {
        assert!(
            result.is_ok(),
            "Stream item {i} should be Ok, but got: {result:?}"
        );
    }
}

// ==================== Test 2: Failure isolation between committees ====================

/// Test 2: A stuck committee does not prevent another committee from producing signed
/// attestations via multi-operator QBFT consensus.
///
/// This test verifies failure isolation in the `FuturesUnordered` fan-out. Committee A has
/// multi-operator QBFT enabled (all 4 operators participate), so consensus succeeds and
/// signed attestations are produced. Committee B's attestations use an unseeded slot, so
/// `get_voting_context` blocks indefinitely.
///
/// The stream should yield committee A's result (with actual signed attestations) without
/// waiting for committee B. Since committee B is permanently blocked, we expect exactly
/// 1 stream item within our timeout.
#[tokio::test(flavor = "multi_thread")]
async fn test_stuck_committee_does_not_block_successful_consensus() {
    // Arrange
    let rsa_private_key = generate_rsa_keypair();
    let rsa_pubkey = rsa_public_from_private(&rsa_private_key);
    let our_operator_id = OperatorId(1);

    let operator_ids_a: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
    let operator_ids_b: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)];

    let committee_a = create_committee_setup(&operator_ids_a, 2, our_operator_id, &rsa_pubkey, 0);
    let committee_b = create_committee_setup(&operator_ids_b, 1, our_operator_id, &rsa_pubkey, 100);

    let committee_id_a = committee_a.cluster.committee_id();

    let (executor, _signal) = create_test_executor();

    let mut harness = ValidatorStoreTestHarness::new(
        vec![committee_a, committee_b],
        our_operator_id,
        rsa_private_key,
        executor,
    );

    // Only seed VotingContext for TEST_SLOT. Committee B's attestation will use a
    // different slot, so its `get_voting_context` call will block forever.
    harness.seed_voting_context();

    let beacon_vote = ssv_types::consensus::BeaconVote {
        block_root: types::Hash256::zero(),
        source: types::Checkpoint {
            epoch: types::Epoch::new(0),
            root: types::Hash256::zero(),
        },
        target: types::Checkpoint {
            epoch: types::Epoch::new(0),
            root: types::Hash256::zero(),
        },
    };

    // Enable multi-operator consensus for committee A
    let mut enabled = HashSet::new();
    enabled.insert(committee_id_a);

    let _captured = harness.start_consensus_router(beacon_vote, &enabled);

    // Committee A attestations use the seeded slot (TEST_SLOT).
    // Committee B attestation uses a slot with no VotingContext, causing it to hang.
    let unseeded_slot = TEST_SLOT + 999;
    let attestations = vec![
        harness.create_attestation_to_sign(0, 0),
        harness.create_attestation_to_sign(0, 1),
        harness.create_attestation_to_sign_at_slot(1, 0, unseeded_slot),
    ];

    // Act
    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    // Collect with a timeout. Committee A should complete (QBFT succeeds with all 4 operators).
    // Committee B blocks on VotingContext, so the stream never fully drains. We expect
    // exactly 1 item (committee A) before our timeout.
    let mut results: SignAttestationsResult = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(item) = stream.next().await {
            results.push(item);
        }
    })
    .await;

    // Assert: committee A produced its stream item with REAL signed attestations
    assert!(
        !results.is_empty(),
        "Expected at least 1 stream item from committee A, but got none. \
         Committee B's hang may have blocked the entire stream."
    );

    assert_eq!(
        results.len(),
        1,
        "Expected exactly 1 stream item (committee A). Committee B should still be stuck. \
         Got {} items.",
        results.len()
    );

    let signed_attestations = results[0]
        .as_ref()
        .expect("Committee A's stream item should be Ok");

    // Committee A had 2 validators with multi-operator QBFT, so we expect 2 signed attestations
    assert_eq!(
        signed_attestations.len(),
        2,
        "Committee A should produce 2 signed attestations (one per validator) via multi-operator \
         QBFT consensus, got {}. This confirms real consensus succeeded, not just a timeout.",
        signed_attestations.len()
    );
}

// ==================== Test 3: Committee collection mode verification ====================

/// Test 3: Partial signature messages use `DutyExecutor::Committee` and batch all validators'
/// signatures into a single outgoing message.
///
/// When `sign_committee_attestations` collects signatures for multiple validators in the same
/// committee, the `SignatureCollectorManager` batches them into a single
/// `PartialSignatureMessages` message with `DutyExecutor::Committee(committee_id)` in the
/// `MessageId`. This test verifies that batching behavior by inspecting the captured outgoing
/// partial signature messages.
#[tokio::test(flavor = "multi_thread")]
async fn test_committee_attestation_uses_committee_collection_mode() {
    // Arrange
    let rsa_private_key = generate_rsa_keypair();
    let rsa_pubkey = rsa_public_from_private(&rsa_private_key);
    let our_operator_id = OperatorId(1);

    let operator_ids: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];

    let num_validators = 3;
    let committee = create_committee_setup(
        &operator_ids,
        num_validators,
        our_operator_id,
        &rsa_pubkey,
        0,
    );
    let committee_id = committee.cluster.committee_id();

    let (executor, _signal) = create_test_executor();

    let mut harness =
        ValidatorStoreTestHarness::new(vec![committee], our_operator_id, rsa_private_key, executor);

    harness.seed_voting_context();

    let beacon_vote = ssv_types::consensus::BeaconVote {
        block_root: types::Hash256::zero(),
        source: types::Checkpoint {
            epoch: types::Epoch::new(0),
            root: types::Hash256::zero(),
        },
        target: types::Checkpoint {
            epoch: types::Epoch::new(0),
            root: types::Hash256::zero(),
        },
    };

    // Enable multi-operator consensus for this committee
    let mut enabled = HashSet::new();
    enabled.insert(committee_id);

    let captured = harness.start_consensus_router(beacon_vote, &enabled);

    let attestations = vec![
        harness.create_attestation_to_sign(0, 0),
        harness.create_attestation_to_sign(0, 1),
        harness.create_attestation_to_sign(0, 2),
    ];

    // Act
    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    let mut results: SignAttestationsResult = Vec::new();
    let collect_timeout = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(item) = stream.next().await {
            results.push(item);
        }
    })
    .await;

    // Assert: stream completed and produced results
    if collect_timeout.is_err() {
        panic!(
            "Stream collection timed out after 30s. Got {} items before timeout.",
            results.len()
        );
    }

    assert_eq!(
        results.len(),
        1,
        "Should have exactly 1 stream item (1 committee), got {}",
        results.len()
    );

    let signed_attestations = results[0].as_ref().expect("stream item should be Ok");
    assert_eq!(
        signed_attestations.len(),
        num_validators,
        "Should have {num_validators} signed attestations, got {}",
        signed_attestations.len()
    );

    // Inspect captured partial signature messages from our operator
    let captured_msgs = captured.lock();

    // Our operator should have sent exactly 1 batched partial signature message
    // (not 3 separate messages, one per validator).
    assert_eq!(
        captured_msgs.len(),
        1,
        "Expected exactly 1 batched partial signature message from our operator, got {}. \
         Committee collection mode should batch all validators' sigs into one message.",
        captured_msgs.len()
    );

    let msg = &captured_msgs[0];

    // Verify the message uses `DutyExecutor::Committee`
    let msg_id = msg.ssv_message().msg_id();
    let duty_executor = msg_id
        .duty_executor()
        .expect("message should have a duty executor");
    match duty_executor {
        ssv_types::msgid::DutyExecutor::Committee(cid) => {
            assert_eq!(
                cid, committee_id,
                "Partial sig message should use the committee's CommitteeId"
            );
        }
        other => {
            panic!(
                "Expected DutyExecutor::Committee, got {other:?}. \
                 Committee attestation sigs should use committee collection mode."
            );
        }
    }

    // Verify the message contains all 3 validators' partial signatures
    let partial_sigs =
        ssv_types::partial_sig::PartialSignatureMessages::from_ssz_bytes(msg.ssv_message().data())
            .expect("should decode partial signature messages");

    assert_eq!(
        partial_sigs.messages.len(),
        num_validators,
        "Batched message should contain {num_validators} partial signatures (one per validator), \
         got {}",
        partial_sigs.messages.len()
    );

    // Verify each partial signature references a distinct validator index
    let mut validator_indices: Vec<_> = partial_sigs
        .messages
        .iter()
        .map(|m| *m.validator_index)
        .collect();
    validator_indices.sort();
    validator_indices.dedup();
    assert_eq!(
        validator_indices.len(),
        num_validators,
        "Each partial signature should be for a distinct validator, but got duplicates"
    );
}

// ==================== Test 4: Not-synced early exit ====================

/// Test 4: When the node is not synced, `sign_attestations` returns an immediate error without
/// entering the committee-batching or QBFT consensus path.
///
/// This verifies the early-exit guard at the top of `sign_attestations`. The stream should
/// yield exactly one `Err(NotSynced)` item regardless of how many committees or validators
/// are involved.
#[tokio::test(flavor = "multi_thread")]
async fn test_not_synced_returns_immediate_error() {
    // Arrange
    let rsa_private_key = generate_rsa_keypair();
    let rsa_pubkey = rsa_public_from_private(&rsa_private_key);
    let our_operator_id = OperatorId(1);

    let operator_ids_a: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
    let operator_ids_b: Vec<OperatorId> =
        vec![OperatorId(1), OperatorId(5), OperatorId(6), OperatorId(7)];

    let committee_a = create_committee_setup(&operator_ids_a, 2, our_operator_id, &rsa_pubkey, 0);
    let committee_b = create_committee_setup(&operator_ids_b, 1, our_operator_id, &rsa_pubkey, 100);

    let (executor, _signal) = create_test_executor();

    let harness = ValidatorStoreTestHarness::new(
        vec![committee_a, committee_b],
        our_operator_id,
        rsa_private_key,
        executor,
    );

    harness.seed_voting_context();

    // Mark node as not synced
    harness.is_synced_tx.send_replace(false);

    let attestations = vec![
        harness.create_attestation_to_sign(0, 0),
        harness.create_attestation_to_sign(0, 1),
        harness.create_attestation_to_sign(1, 0),
    ];

    // Act
    let stream = harness.validator_store.sign_attestations(attestations);
    tokio::pin!(stream);

    let mut results: SignAttestationsResult = Vec::new();
    // This should return instantly (no QBFT involved), so a short timeout is fine.
    let collect_timeout = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(item) = stream.next().await {
            results.push(item);
        }
    })
    .await;

    // Assert
    assert!(
        collect_timeout.is_ok(),
        "Not-synced path should return immediately, not time out"
    );

    assert_eq!(
        results.len(),
        1,
        "Not-synced should produce exactly 1 error item, got {}.",
        results.len()
    );

    let err = results[0]
        .as_ref()
        .expect_err("Stream item should be Err when not synced");

    assert!(
        matches!(
            err,
            validator_store::Error::SpecificError(SpecificError::NotSynced)
        ),
        "Error should be NotSynced, but got: {err:?}"
    );
}
