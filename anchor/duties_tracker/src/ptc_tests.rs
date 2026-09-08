//! Exercise the PTC HTTP client, request coverage and installed assignment snapshot together.

use std::{sync::Mutex, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use beacon_node_fallback::{ApiTopic, CandidateBeaconNode, Config};
use database::{
    NetworkDatabase, PendingStateUpdates,
    test_utils::{InMemoryTestFixture, commit_and_publish, generators},
};
use eth2::{BeaconNodeHttpClient, Timeouts, types::PtcDuty};
use sensitive_url::SensitiveUrl;
use serde_json::{Value, json};
use slot_clock::ManualSlotClock;
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use types::Hash256;

use super::*;

const SLOTS_PER_EPOCH: u64 = 32;
const TEST_EPOCH: u64 = 3;
const TEST_INDEX: u64 = 10;
const SLOT_DURATION: Duration = Duration::from_secs(12);
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

type Request = (u64, Vec<String>);
type Response = (StatusCode, Value);

struct HttpState {
    response: Mutex<Response>,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    requests: mpsc::UnboundedSender<Request>,
}

struct PtcServer {
    state: Arc<HttpState>,
    requests: mpsc::UnboundedReceiver<Request>,
    task: JoinHandle<()>,
    url: SensitiveUrl,
}

impl PtcServer {
    async fn new() -> Self {
        let (tx, requests) = mpsc::unbounded_channel();
        let state = Arc::new(HttpState {
            response: Mutex::new((StatusCode::OK, response(vec![]))),
            gate: Mutex::new(None),
            requests: tx,
        });
        let app = Router::new()
            .route("/eth/v1/validator/duties/ptc/{epoch}", post(serve_ptc))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url =
            SensitiveUrl::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            state,
            requests,
            task,
            url,
        }
    }

    fn respond(&self, status: StatusCode, body: Value) {
        *self.state.response.lock().unwrap() = (status, body);
    }

    fn pause_response(&self) -> oneshot::Sender<()> {
        let (tx, rx) = oneshot::channel();
        *self.state.gate.lock().unwrap() = Some(rx);
        tx
    }

    fn take_requests(&mut self) -> Vec<Request> {
        let mut requests = vec![];
        while let Ok(request) = self.requests.try_recv() {
            requests.push(request);
        }
        requests
    }
}

impl Drop for PtcServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_ptc(
    State(state): State<Arc<HttpState>>,
    Path(epoch): Path<u64>,
    Json(indices): Json<Vec<String>>,
) -> (StatusCode, Json<Value>) {
    let response = state.response.lock().unwrap().clone();
    let gate = state.gate.lock().unwrap().take();
    state.requests.send((epoch, indices)).unwrap();
    if let Some(gate) = gate {
        gate.await.unwrap();
    }
    (response.0, Json(response.1))
}

fn tracker(
    server: &PtcServer,
    db: &NetworkDatabase,
    gloas_epoch: Option<u64>,
) -> DutiesTracker<ManualSlotClock> {
    let mut spec = ChainSpec::mainnet();
    spec.gloas_fork_epoch = gloas_epoch.map(Epoch::new);
    let spec = Arc::new(spec);
    let client = BeaconNodeHttpClient::new(server.url.clone(), Timeouts::set_all(TEST_TIMEOUT));
    let beacon_nodes = Arc::new(BeaconNodeFallback::new(
        vec![CandidateBeaconNode::new(client, 0)],
        Config::default(),
        ApiTopic::all(),
        spec.clone(),
    ));
    let clock = ManualSlotClock::new(Slot::new(0), Duration::ZERO, SLOT_DURATION);
    clock.set_slot(TEST_EPOCH * SLOTS_PER_EPOCH);
    DutiesTracker::new(
        Arc::new(VoluntaryExitTracker::new()),
        beacon_nodes,
        spec,
        SLOTS_PER_EPOCH,
        clock,
        db.watch(),
    )
}

/// Metadata without our own shares models validators whose messages Anchor only relays.
fn register_index(db: &NetworkDatabase, index: u64) -> PublicKeyBytes {
    let cluster = generators::cluster::random(0);
    let mut validator = generators::validator::random_metadata(cluster.cluster_id);
    validator.index = Some(validator_index(index));
    let mut conn = db.connection().unwrap();
    let tx = conn.transaction().unwrap();
    let mut pending = PendingStateUpdates::default();
    db.insert_validator_tx(cluster, &validator, vec![], &tx, &mut pending)
        .unwrap();
    commit_and_publish(db, tx, pending);
    validator.public_key
}

fn duty(index: u64, slot: Slot) -> PtcDuty {
    PtcDuty {
        pubkey: generators::pubkey::random(),
        validator_index: index,
        slot,
    }
}

fn response(data: Vec<PtcDuty>) -> Value {
    serde_json::to_value(DutiesResponse {
        dependent_root: Hash256::from([0; 32]),
        execution_optimistic: Some(false),
        data,
    })
    .unwrap()
}

fn validator_index(index: u64) -> ValidatorIndex {
    ValidatorIndex(usize::try_from(index).unwrap())
}

fn test_slot() -> Slot {
    Epoch::new(TEST_EPOCH).start_slot(SLOTS_PER_EPOCH)
}

#[tokio::test]
async fn test_ptc_poll_covers_relayed_validators_and_replaces_whole_snapshot() {
    // Arrange: all three indexed validators are known, and none has a locally owned share.
    let fixture = InMemoryTestFixture::new_empty();
    for index in TEST_INDEX..TEST_INDEX + 3 {
        register_index(&fixture.db, index);
    }
    assert!(fixture.db.state().shares().values().next().is_none());
    let mut server = PtcServer::new().await;
    let tracker = tracker(&server, &fixture.db, Some(TEST_EPOCH));
    let slot = test_slot();
    server.respond(
        StatusCode::OK,
        response(vec![duty(TEST_INDEX, slot), duty(TEST_INDEX + 1, slot + 1)]),
    );

    // Act: the actual sparse HTTP response supplies assignments for only two requested indices.
    tracker.poll_ptc_duties().await.unwrap();

    // Assert: wrong-slot and omitted queried indices are negatives, unqueried indices are unknown.
    for (query_slot, index, expected) in [
        (slot, TEST_INDEX, DutyAssignment::Assigned),
        (slot, TEST_INDEX + 1, DutyAssignment::NotAssigned),
        (slot, TEST_INDEX + 2, DutyAssignment::NotAssigned),
        (slot, TEST_INDEX + 3, DutyAssignment::Unknown),
        (slot + SLOTS_PER_EPOCH, TEST_INDEX, DutyAssignment::Unknown),
    ] {
        assert_eq!(
            tracker.ptc_assignment_at_slot(query_slot, validator_index(index)),
            expected,
            "slot {query_slot}, validator {index}"
        );
    }
    let mut requests = server.take_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, TEST_EPOCH);
    requests[0].1.sort();
    assert_eq!(
        requests[0].1,
        (TEST_INDEX..TEST_INDEX + 3)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
    );

    // Act: a complete refresh changes the roster without retaining old positive rows.
    server.respond(StatusCode::OK, response(vec![duty(TEST_INDEX + 2, slot)]));
    tracker.poll_ptc_duties().await.unwrap();

    // Assert: the old assigned validator is now known absent, and the new assignment is installed.
    assert_eq!(
        tracker.ptc_assignment_at_slot(slot, validator_index(TEST_INDEX)),
        DutyAssignment::NotAssigned
    );
    assert_eq!(
        tracker.ptc_assignment_at_slot(slot, validator_index(TEST_INDEX + 2)),
        DutyAssignment::Assigned
    );
}

#[tokio::test]
async fn test_ptc_poll_registration_during_request_stays_unknown_until_queried() {
    // Arrange: pause the HTTP response after its request has reached the local server.
    let fixture = InMemoryTestFixture::new_empty();
    register_index(&fixture.db, TEST_INDEX);
    let mut server = PtcServer::new().await;
    let release = server.pause_response();
    let tracker = Arc::new(tracker(&server, &fixture.db, Some(TEST_EPOCH)));
    let polling = tokio::spawn({
        let tracker = tracker.clone();
        async move { tracker.poll_ptc_duties().await }
    });
    let request = tokio::time::timeout(TEST_TIMEOUT, server.requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request, (TEST_EPOCH, vec![TEST_INDEX.to_string()]));

    // Act: publish another validator while the old request remains pending, then finish it.
    register_index(&fixture.db, TEST_INDEX + 1);
    release.send(()).unwrap();
    polling.await.unwrap().unwrap();

    // Assert: a successful empty response cannot establish absence for the new index.
    assert_eq!(
        tracker.ptc_assignment_at_slot(test_slot(), validator_index(TEST_INDEX)),
        DutyAssignment::NotAssigned
    );
    assert_eq!(
        tracker.ptc_assignment_at_slot(test_slot(), validator_index(TEST_INDEX + 1)),
        DutyAssignment::Unknown
    );

    // Act: the following complete poll includes the newly registered index.
    tracker.poll_ptc_duties().await.unwrap();

    // Assert: only this response can establish its absence.
    assert_eq!(
        tracker.ptc_assignment_at_slot(test_slot(), validator_index(TEST_INDEX + 1)),
        DutyAssignment::NotAssigned
    );
}

#[tokio::test]
async fn test_ptc_poll_failed_or_malformed_response_preserves_prior_knowledge() {
    // Arrange: cover transport/API errors and responses that cannot define a coherent snapshot.
    let fixture = InMemoryTestFixture::new_empty();
    register_index(&fixture.db, TEST_INDEX);
    let server = PtcServer::new().await;
    let tracker = tracker(&server, &fixture.db, Some(TEST_EPOCH));
    let slot = test_slot();
    let invalid_responses = [
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"code": 500, "message": "unavailable"}),
        ),
        (StatusCode::OK, json!({"invalid": "duties response"})),
        (
            StatusCode::OK,
            response(vec![duty(TEST_INDEX, slot + SLOTS_PER_EPOCH)]),
        ),
        (StatusCode::OK, response(vec![duty(TEST_INDEX + 1, slot)])),
        (
            StatusCode::OK,
            response(vec![duty(TEST_INDEX, slot), duty(TEST_INDEX, slot + 1)]),
        ),
        (
            StatusCode::OK,
            response(vec![duty(TEST_INDEX, slot), duty(TEST_INDEX, slot)]),
        ),
    ];

    for expected in [DutyAssignment::Unknown, DutyAssignment::Assigned] {
        if expected == DutyAssignment::Assigned {
            server.respond(StatusCode::OK, response(vec![duty(TEST_INDEX, slot)]));
            tracker.poll_ptc_duties().await.unwrap();
        }
        for (status, body) in &invalid_responses {
            // Act: neither failed initialization nor failed refresh is a valid negative.
            server.respond(*status, body.clone());
            assert!(tracker.poll_ptc_duties().await.is_err());

            // Assert: no malformed row or omitted response revokes prior knowledge.
            assert_eq!(
                tracker.ptc_assignment_at_slot(slot, validator_index(TEST_INDEX)),
                expected
            );
        }
    }
}

#[tokio::test]
async fn test_ptc_poll_empty_index_set_clears_current_snapshot_without_http() {
    // Arrange: first fetch a valid current-epoch snapshot.
    let fixture = InMemoryTestFixture::new_empty();
    let pubkey = register_index(&fixture.db, TEST_INDEX);
    let mut server = PtcServer::new().await;
    let tracker = tracker(&server, &fixture.db, Some(TEST_EPOCH));
    tracker.poll_ptc_duties().await.unwrap();
    assert_eq!(
        tracker.ptc_assignment_at_slot(test_slot(), validator_index(TEST_INDEX)),
        DutyAssignment::NotAssigned
    );
    server.take_requests();

    // Act: remove the last registered index and poll again.
    let mut conn = fixture.db.connection().unwrap();
    let tx = conn.transaction().unwrap();
    let mut pending = PendingStateUpdates::default();
    fixture
        .db
        .delete_validator_tx(&pubkey, &tx, &mut pending)
        .unwrap();
    commit_and_publish(&fixture.db, tx, pending);
    tracker.poll_ptc_duties().await.unwrap();

    // Assert: empty coverage is unknown and the API is never called with an empty index list.
    assert_eq!(
        tracker.ptc_assignment_at_slot(test_slot(), validator_index(TEST_INDEX)),
        DutyAssignment::Unknown
    );
    assert!(server.take_requests().is_empty());
}

#[tokio::test]
async fn test_ptc_poll_gloas_gate_and_previous_epoch_retention_on_failure() {
    // Arrange: a known index must not trigger requests before Gloas or if it is unscheduled.
    let fixture = InMemoryTestFixture::new_empty();
    register_index(&fixture.db, TEST_INDEX);
    let mut server = PtcServer::new().await;
    for gloas_epoch in [None, Some(TEST_EPOCH + 1)] {
        let tracker = tracker(&server, &fixture.db, gloas_epoch);
        tracker.poll_ptc_duties().await.unwrap();
        assert_eq!(
            tracker.ptc_assignment_at_slot(test_slot(), validator_index(TEST_INDEX)),
            DutyAssignment::Unknown
        );
    }
    assert!(server.take_requests().is_empty());
    let tracker = tracker(&server, &fixture.db, Some(TEST_EPOCH));

    // Act: fetch current epochs at activation and one epoch later, then fail the next refresh.
    for epoch in [TEST_EPOCH, TEST_EPOCH + 1] {
        let slot = Epoch::new(epoch).start_slot(SLOTS_PER_EPOCH);
        tracker.slot_clock.set_slot(slot.as_u64());
        server.respond(StatusCode::OK, response(vec![duty(TEST_INDEX, slot)]));
        tracker.poll_ptc_duties().await.unwrap();
    }
    assert_eq!(
        tracker.ptc_assignment_at_slot(test_slot(), validator_index(TEST_INDEX)),
        DutyAssignment::Assigned
    );
    tracker
        .slot_clock
        .set_slot((TEST_EPOCH + 2) * SLOTS_PER_EPOCH);
    server.respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"code": 500, "message": "unavailable"}),
    );
    assert!(tracker.poll_ptc_duties().await.is_err());

    // Assert: pruning still runs on failure, keeps the previous epoch, and never queries lookahead.
    for (slot, expected) in [
        (test_slot(), DutyAssignment::Unknown),
        (test_slot() + SLOTS_PER_EPOCH, DutyAssignment::Assigned),
        (test_slot() + 2 * SLOTS_PER_EPOCH, DutyAssignment::Unknown),
    ] {
        assert_eq!(
            tracker.ptc_assignment_at_slot(slot, validator_index(TEST_INDEX)),
            expected,
            "slot {slot}"
        );
    }
    let requests = server.take_requests();
    let mut queried_epochs = requests.iter().map(|(epoch, _)| *epoch).collect::<Vec<_>>();
    queried_epochs.dedup();
    assert_eq!(
        queried_epochs,
        vec![TEST_EPOCH, TEST_EPOCH + 1, TEST_EPOCH + 2]
    );
}
