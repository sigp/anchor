//! PTC assignment validation through the complete partial-signature admission pipeline.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{Json, Router, routing::post};
use beacon_node_fallback::{ApiTopic, BeaconNodeFallback, CandidateBeaconNode, Config};
use database::{
    PendingStateUpdates,
    test_utils::{InMemoryTestFixture, generators},
};
use duties_tracker::{
    DutiesProvider, DutyAssignment, duties_tracker::DutiesTracker,
    voluntary_exit_tracker::VoluntaryExitTracker,
};
use eth2::{BeaconNodeHttpClient, Timeouts, types::DutiesResponse};
use fork::Fork;
use sensitive_url::SensitiveUrl;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    CommitteeInfo, OperatorId, ValidatorIndex, message::SignedSSVMessage, msgid::Role,
};
use task_executor::test_utils::TestRuntime;
use types::{Epoch, Hash256, Slot};

use super::{
    tests::{PartialSigTestOptions, create_test_partial_signature},
    *,
};
use crate::{
    MessageAcceptance,
    tests::{
        MockDutiesProvider, four_node_committee_and_keypair, generate_fork_schedule,
        spec_with_gloas,
    },
};

const SLOTS_PER_EPOCH: u64 = 32;
const SLOT_DURATION: Duration = Duration::from_secs(12);
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const SIGNER: OperatorId = OperatorId(1);

fn context<'a>(
    message: &'a SignedSSVMessage,
    committee: &'a CommitteeInfo,
    keys: &'a HashMap<OperatorId, openssl::rsa::Rsa<openssl::pkey::Public>>,
) -> ValidationContext<'a, ManualSlotClock> {
    let now = SystemTime::now();
    ValidationContext {
        signed_ssv_message: message,
        committee_info: committee,
        role: Role::PTCAttester,
        received_at: now,
        slots_per_epoch: SLOTS_PER_EPOCH,
        epochs_per_sync_committee_period: 256,
        sync_committee_size: 512,
        slot_clock: ManualSlotClock::new(
            Slot::new(0),
            now.duration_since(UNIX_EPOCH).unwrap(),
            SLOT_DURATION,
        ),
        operator_pub_keys: keys,
        fork_schedule: generate_fork_schedule(Fork::Boole),
        spec: spec_with_gloas(Some(0)),
    }
}

#[test]
fn test_ptc_not_assigned_ignored_without_consuming_accepted_message_state() {
    // Arrange: a correctly signed PTC message and a fetched view proving no assignment.
    let (mut committee, private_key, keys) = four_node_committee_and_keypair();
    committee.validator_indices = vec![ValidatorIndex(0)];
    let (_, message) = create_test_partial_signature(
        Role::PTCAttester,
        PartialSignatureKind::PTCAttester,
        SIGNER,
        PartialSigTestOptions::default(),
        Some(private_key),
    );
    let mut state = DutyState::new(2 * SLOTS_PER_EPOCH as usize);

    // Act: exercise role, index, assignment, signature and state handling in the real pipeline.
    let failure = validate_partial_signature_message(
        context(&message, &committee, &keys),
        &mut state,
        Arc::new(MockDutiesProvider {
            ptc_assignment: DutyAssignment::NotAssigned,
            ..Default::default()
        }),
    )
    .unwrap_err();

    // Assert: operator allocation is allowed, but no accepted slot or message budget is recorded.
    assert!(matches!(failure, ValidationFailure::NoDuty));
    assert!(matches!(
        MessageAcceptance::from(&failure),
        MessageAcceptance::Ignore
    ));
    let operator = state.get_or_create_operator(&SIGNER);
    assert!(operator.get_signer_state(&Slot::new(0)).is_none());
    assert_eq!(operator.get_duty_count(Epoch::new(0), SLOTS_PER_EPOCH), 0);

    // Act: accept the identical message after a refreshed view confirms its assignment.
    let accepted = validate_partial_signature_message(
        context(&message, &committee, &keys),
        &mut state,
        Arc::new(MockDutiesProvider {
            ptc_assignment: DutyAssignment::Assigned,
            ..Default::default()
        }),
    );

    // Assert: the ignored attempt did not consume the slot's message-count allowance.
    assert!(
        accepted.is_ok(),
        "assignment rejection consumed state: {accepted:?}"
    );
    assert_eq!(
        state
            .get_or_create_operator(&SIGNER)
            .get_duty_count(Epoch::new(0), SLOTS_PER_EPOCH),
        1
    );
}

#[test]
fn test_ptc_unknown_view_and_unresolved_local_index_continue_other_validation() {
    // Arrange: locally missing metadata must not trust a purported known-negative index lookup.
    let (mut committee, private_key, keys) = four_node_committee_and_keypair();
    let (_, message) = create_test_partial_signature(
        Role::PTCAttester,
        PartialSignatureKind::PTCAttester,
        SIGNER,
        PartialSigTestOptions::default(),
        Some(private_key),
    );
    for (indices, assignment) in [
        (vec![ValidatorIndex(0)], DutyAssignment::Unknown),
        (vec![], DutyAssignment::NotAssigned),
    ] {
        committee.validator_indices = indices;

        // Act: both unknown duty coverage and an unresolved local index bypass only assignment.
        let result = validate_partial_signature_message(
            context(&message, &committee, &keys),
            &mut DutyState::new(2 * SLOTS_PER_EPOCH as usize),
            Arc::new(MockDutiesProvider {
                ptc_assignment: assignment,
                ..Default::default()
            }),
        );

        // Assert: correctly signed messages remain admissible in both cases.
        assert!(
            result.is_ok(),
            "unknown assignment was treated as no duty: {result:?}"
        );
    }

    // Act: unknown assignment still passes through RSA verification.
    committee.validator_indices = vec![ValidatorIndex(0)];
    let (_, unsigned_message) = create_test_partial_signature(
        Role::PTCAttester,
        PartialSignatureKind::PTCAttester,
        SIGNER,
        PartialSigTestOptions::default(),
        None,
    );
    let result = validate_partial_signature_message(
        context(&unsigned_message, &committee, &keys),
        &mut DutyState::new(2 * SLOTS_PER_EPOCH as usize),
        Arc::new(MockDutiesProvider {
            ptc_assignment: DutyAssignment::Unknown,
            ..Default::default()
        }),
    );

    // Assert: assignment uncertainty does not turn off signature validation.
    assert!(matches!(
        result,
        Err(ValidationFailure::SignatureVerificationFailed { .. })
    ));
}

/// Starts the real polling task against a local Beacon API fixture, then supplies that exact
/// tracker to partial-signature validation. This pins the fetch-to-admission connection that
/// independent provider mocks cannot establish.
#[tokio::test]
async fn test_ptc_started_tracker_http_snapshot_controls_partial_signature_admission() {
    // Arrange: indices 0, 1 and 2 are registered; index 3 has no fetched coverage.
    let fixture = InMemoryTestFixture::new_empty();
    let mut duties = vec![];
    for index in 0..3 {
        let cluster = generators::cluster::random(0);
        let mut validator = generators::validator::random_metadata(cluster.cluster_id);
        validator.index = Some(ValidatorIndex(index));
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();
        fixture
            .db
            .insert_validator_tx(cluster, &validator, vec![], &tx, &mut pending)
            .unwrap();
        tx.commit().unwrap();
        fixture.db.publish_pending_state_updates(pending);
        if index < 2 {
            duties.push(eth2::types::PtcDuty {
                pubkey: validator.public_key,
                validator_index: u64::try_from(index).unwrap(),
                slot: Slot::new(u64::try_from(index).unwrap()),
            });
        }
    }
    let response = serde_json::to_value(DutiesResponse {
        dependent_root: Hash256::from([0; 32]),
        execution_optimistic: Some(false),
        data: duties,
    })
    .unwrap();
    let app = Router::new().route(
        "/eth/v1/validator/duties/ptc/0",
        post(move |Json(mut indices): Json<Vec<String>>| {
            let response = response.clone();
            async move {
                indices.sort();
                assert_eq!(indices, vec!["0", "1", "2"]);
                Json(response)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = SensitiveUrl::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let spec = spec_with_gloas(Some(0));
    let client = BeaconNodeHttpClient::new(url, Timeouts::set_all(TEST_TIMEOUT));
    let beacon_nodes = Arc::new(BeaconNodeFallback::new(
        vec![CandidateBeaconNode::new(client, 0)],
        Config::default(),
        ApiTopic::all(),
        spec.clone(),
    ));
    let tracker = Arc::new(DutiesTracker::new(
        Arc::new(VoluntaryExitTracker::new()),
        beacon_nodes,
        spec,
        SLOTS_PER_EPOCH,
        ManualSlotClock::new(Slot::new(0), Duration::ZERO, SLOT_DURATION),
        fixture.db.watch(),
    ));
    let runtime = TestRuntime::default();

    // Act: start production polling, waiting on its public assignment query rather than a delay.
    tracker.clone().start(runtime.task_executor.clone());
    tokio::time::timeout(TEST_TIMEOUT, async {
        while tracker.ptc_assignment_at_slot(Slot::new(0), ValidatorIndex(0))
            != DutyAssignment::Assigned
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("started tracker must install the HTTP PTC snapshot");
    let (mut committee, private_key, keys) = four_node_committee_and_keypair();
    for (index, should_accept) in [(0, true), (1, false), (2, false), (3, true)] {
        committee.validator_indices = vec![ValidatorIndex(index)];
        let (_, message) = create_test_partial_signature(
            Role::PTCAttester,
            PartialSignatureKind::PTCAttester,
            SIGNER,
            PartialSigTestOptions {
                validator_index: Some(ValidatorIndex(index)),
                ..Default::default()
            },
            Some(private_key.clone()),
        );
        let result = validate_partial_signature_message(
            context(&message, &committee, &keys),
            &mut DutyState::new(2 * SLOTS_PER_EPOCH as usize),
            tracker.clone(),
        );

        // Assert: assigned/unqueried pass; wrong-slot/queried-absent are ignored.
        if should_accept {
            assert!(result.is_ok(), "index {index}: {result:?}");
        } else {
            let failure = result.unwrap_err();
            assert!(
                matches!(failure, ValidationFailure::NoDuty),
                "index {index}: {failure:?}"
            );
            assert!(matches!(
                MessageAcceptance::from(&failure),
                MessageAcceptance::Ignore
            ));
        }
    }
    server.abort();
}
