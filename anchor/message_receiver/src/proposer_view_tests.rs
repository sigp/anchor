//! Proposer-preference delivery with conflicting receive-side and local proposer evidence.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{Json, Router, extract::Path, routing::get};
use beacon_node_fallback::{ApiTopic, BeaconNodeFallback, CandidateBeaconNode, Config};
use bls::{PublicKeyBytes, SecretKey};
use bls_lagrange::{KeyId, split};
use database::{
    OwnOperatorId, PendingStateUpdates, UniqueIndex,
    test_utils::{InMemoryTestFixture, TEST_NETWORK, commit_and_publish, generators},
};
use duties_tracker::{
    DutiesProvider, DutyAssignment, duties_tracker::DutiesTracker,
    voluntary_exit_tracker::VoluntaryExitTracker,
};
use eth2::{
    BeaconNodeHttpClient, Timeouts,
    types::{DutiesResponse, ProposerData},
};
use fork::{Fork, ForkLifecycle, ForkSchedule};
use libp2p::{
    PeerId,
    gossipsub::{IdentTopic, Message, MessageAcceptance, MessageId},
};
use message_sender::testing::MockMessageSender;
use message_validator::{ValidationFailure, Validator};
use openssl::{
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Rsa,
    sign::Signer,
};
use sensitive_url::SensitiveUrl;
use signature_collector::{
    SignatureCollectorManager, SignatureMetadata, SignatureRequester, ValidatorSigningData,
};
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    Operator, OperatorId, ValidatorIndex, VariableList,
    domain_type::DomainType,
    message::{MsgType, SSVMessage, SignedSSVMessage},
    msgid::{DutyExecutor, MessageId as SsvMessageId, Role},
    partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
};
use ssz::Encode;
use subnet_service::{start_subnet_service, topic::parse_topic};
use task_executor::test_utils::TestRuntime;
use tokio::sync::{mpsc, oneshot, watch};
use types::{Epoch, Hash256, MainnetEthSpec, Slot};

use crate::{MessageReceiver, NetworkMessageReceiver, TopicContext};

const SLOTS_PER_EPOCH: u64 = 32;
const SLOT_DURATION: Duration = Duration::from_secs(12);
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const QUIESCENCE_TIMEOUT: Duration = Duration::from_millis(1);
const DUTY_SLOT: Slot = Slot::new(SLOTS_PER_EPOCH);
const VALIDATOR_X: ValidatorIndex = ValidatorIndex(0);
const VALIDATOR_Y: ValidatorIndex = ValidatorIndex(1);
const OWN_SIGNER: OperatorId = OperatorId(1);
const IGNORED_SIGNER: OperatorId = OperatorId(2);
const FRESH_SIGNER: OperatorId = OperatorId(3);
const CONTROL_SIGNER: OperatorId = OperatorId(4);
const THRESHOLD: u64 = 3;
const SIGNING_ROOT: Hash256 = Hash256::repeat_byte(0xAB);

fn partial(signer: OperatorId, key: &SecretKey) -> PartialSignatureMessages {
    PartialSignatureMessages {
        kind: PartialSignatureKind::ProposerPreferences,
        slot: DUTY_SLOT,
        messages: VariableList::new(vec![PartialSignatureMessage {
            partial_signature: key.sign(SIGNING_ROOT),
            signing_root: SIGNING_ROOT,
            signer,
            validator_index: VALIDATOR_X,
        }])
        .unwrap(),
    }
}

fn signed_partial(
    signer: OperatorId,
    key: &SecretKey,
    rsa_key: &PKey<Private>,
    pubkey: PublicKeyBytes,
) -> SignedSSVMessage {
    let message = SSVMessage::new(
        MsgType::SSVPartialSignatureMsgType,
        SsvMessageId::new(
            &DomainType::default(),
            Role::ProposerPreferences,
            &DutyExecutor::Validator(pubkey),
        ),
        partial(signer, key).as_ssz_bytes(),
    )
    .unwrap();
    let mut signature = Signer::new(MessageDigest::sha256(), rsa_key).unwrap();
    signature.update(&message.as_ssz_bytes()).unwrap();
    SignedSSVMessage::new(
        vec![signature.sign_to_vec().unwrap().try_into().unwrap()],
        vec![signer],
        message,
        vec![],
    )
    .unwrap()
}

async fn wait_until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition should become true promptly");
}

/// One urgent worker orders this barrier after receiver and local signing work. Immediate
/// permitless work then orders collector-channel sends before the second barrier.
async fn drain_processor(processor: &processor::Senders) {
    let (urgent_tx, urgent_rx) = oneshot::channel();
    processor
        .urgent_consensus
        .send_blocking(
            move || {
                urgent_tx.send(()).unwrap();
            },
            "proposer_view_urgent_barrier",
        )
        .unwrap();
    tokio::time::timeout(TEST_TIMEOUT, urgent_rx)
        .await
        .unwrap()
        .unwrap();
    let (permitless_tx, permitless_rx) = oneshot::channel();
    processor
        .permitless
        .send_immediate(
            move |_| {
                permitless_tx.send(()).unwrap();
            },
            "proposer_view_permitless_barrier",
        )
        .unwrap();
    tokio::time::timeout(TEST_TIMEOUT, permitless_rx)
        .await
        .unwrap()
        .unwrap();
}

/// Both validators and every operator are registered before polling. The HTTP schedule continues
/// to assign Y while exact local evidence assigns X. This tests receiver-to-collector delivery,
/// not a chain reorg, gossip replay, Lighthouse cache population, or Beacon API publication.
#[tokio::test]
async fn test_proposer_preferences_local_positive_reaches_collector_despite_http_negative() {
    // Arrange: a known four-operator committee with real 3-of-4 BLS shares for validator X.
    let fixture = InMemoryTestFixture::new_empty();
    let master = SecretKey::random();
    let shares = split(
        &master,
        THRESHOLD,
        (1..=4).map(|id| KeyId::try_from(id).unwrap()),
    )
    .unwrap();
    let share = |id: OperatorId| {
        &shares
            .iter()
            .find(|(key_id, _)| u64::from(key_id.clone()) == id.0)
            .unwrap()
            .1
    };
    let mut operators = vec![Operator::new_with_pubkey(
        fixture.pubkey.clone(),
        OWN_SIGNER,
        Default::default(),
    )];
    let rsa_keys = [IGNORED_SIGNER, FRESH_SIGNER].map(|id| {
        let private = Rsa::generate(2048).unwrap();
        let public = Rsa::public_key_from_pem(&private.public_key_to_pem().unwrap()).unwrap();
        operators.push(Operator::new_with_pubkey(public, id, Default::default()));
        PKey::from_rsa(private).unwrap()
    });
    operators.push(generators::operator::with_id(CONTROL_SIGNER.0));
    let cluster = generators::cluster::with_operators(&operators);
    let mut validator_x = generators::validator::random_metadata(cluster.cluster_id);
    validator_x.public_key = master.public_key().compress();
    validator_x.index = Some(VALIDATOR_X);
    let mut validator_y = generators::validator::random_metadata(cluster.cluster_id);
    validator_y.index = Some(VALIDATOR_Y);
    {
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();
        for operator in &operators {
            fixture
                .db
                .insert_operator_tx(operator, &tx, &mut pending)
                .unwrap();
        }
        let database_shares = operators
            .iter()
            .map(|operator| {
                let mut stored = generators::share::random(
                    cluster.cluster_id,
                    operator.id,
                    &validator_x.public_key,
                );
                stored.share_pubkey = share(operator.id).public_key().compress();
                stored
            })
            .collect();
        fixture
            .db
            .insert_validator_tx(
                cluster.clone(),
                &validator_x,
                database_shares,
                &tx,
                &mut pending,
            )
            .unwrap();
        fixture
            .db
            .insert_validator_tx(cluster.clone(), &validator_y, vec![], &tx, &mut pending)
            .unwrap();
        commit_and_publish(&fixture.db, tx, pending);
    }
    let database = Arc::new(fixture.data.db);
    assert!(
        database
            .state()
            .shares()
            .get_by(&validator_x.public_key)
            .is_some(),
        "receiver must own a share for X"
    );
    for pubkey in [validator_x.public_key, validator_y.public_key] {
        assert!(
            database
                .state()
                .get_committee_info_by_validator_pk(&pubkey)
                .is_some()
        );
    }
    let runtime = TestRuntime::default();
    let mut spec = types::ChainSpec::mainnet();
    spec.gloas_fork_epoch = Some(Epoch::new(0));
    let spec = Arc::new(spec);
    let clock = ManualSlotClock::new(
        Slot::new(0),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
        SLOT_DURATION,
    );
    let fork_schedule = Arc::new(ForkSchedule::new(
        Fork::Boole,
        DomainType::default(),
        TEST_NETWORK,
    ));
    let app = Router::new().route(
        "/eth/v2/validator/duties/proposer/{epoch}",
        get(move |Path(epoch): Path<u64>| async move {
            Json(DutiesResponse {
                dependent_root: Hash256::repeat_byte(0xAA),
                execution_optimistic: Some(false),
                data: (epoch * SLOTS_PER_EPOCH..(epoch + 1) * SLOTS_PER_EPOCH)
                    .map(|slot| ProposerData {
                        pubkey: validator_y.public_key,
                        validator_index: u64::from(VALIDATOR_Y),
                        slot: Slot::new(slot),
                    })
                    .collect::<Vec<_>>(),
            })
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = SensitiveUrl::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let beacon_nodes = Arc::new(BeaconNodeFallback::new(
        vec![CandidateBeaconNode::new(
            BeaconNodeHttpClient::new(url, Timeouts::set_all(TEST_TIMEOUT)),
            0,
        )],
        Config::default(),
        ApiTopic::all(),
        spec.clone(),
    ));
    let tracker = Arc::new(DutiesTracker::new(
        Arc::new(VoluntaryExitTracker::new()),
        beacon_nodes,
        spec.clone(),
        SLOTS_PER_EPOCH,
        clock.clone(),
        database.watch(),
    ));
    tracker.clone().start(runtime.task_executor.clone());
    wait_until(|| tracker.is_epoch_known_for_proposers(Epoch::new(1))).await;
    assert_eq!(
        tracker.proposer_assignment_at_slot(DUTY_SLOT, &validator_y.public_key),
        DutyAssignment::Assigned
    );
    assert_eq!(
        tracker.proposer_assignment_at_slot(DUTY_SLOT, &validator_x.public_key),
        DutyAssignment::NotAssigned
    );

    let processor = processor::spawn(
        processor::Config {
            max_workers: 1,
            ..Default::default()
        },
        runtime.task_executor.clone(),
    );
    let (_lifecycle_tx, lifecycle_rx) = watch::channel(ForkLifecycle::Normal {
        current: fork_schedule.active_fork_config(Epoch::new(0)).clone(),
    });
    let (subnets, _topic_events) = start_subnet_service::<_, MainnetEthSpec>(
        database.watch(),
        false,
        true,
        &runtime.task_executor,
        clock.clone(),
        spec.clone(),
        fork_schedule.clone(),
        lifecycle_rx,
    );
    let validator = Validator::new(
        database.watch(),
        SLOTS_PER_EPOCH,
        256,
        512,
        tracker.clone(),
        clock.clone(),
        subnets.clone(),
        fork_schedule.clone(),
        spec.clone(),
        &runtime.task_executor,
    );
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let sender = Arc::new(MockMessageSender::new(outbound_tx, OWN_SIGNER));
    let collector = SignatureCollectorManager::new(
        processor.clone(),
        OwnOperatorId::Known(OWN_SIGNER),
        database.clone(),
        fork_schedule.clone(),
        SLOTS_PER_EPOCH,
        sender.clone(),
        clock.clone(),
    )
    .unwrap();
    let qbft = qbft_manager::QbftManager::<MainnetEthSpec, _>::new(
        processor.clone(),
        OwnOperatorId::Known(OWN_SIGNER),
        clock.clone(),
        sender,
        SLOTS_PER_EPOCH.try_into().unwrap(),
        fork_schedule,
        spec,
    )
    .unwrap();
    let (_synced_tx, synced_rx) = watch::channel(true);
    let (outcome_tx, mut outcomes) = mpsc::channel(2);
    let receiver = NetworkMessageReceiver::new(
        processor.clone(),
        qbft,
        collector.clone(),
        Arc::new(dissemination_store::DisseminationStore::new()),
        database.watch(),
        synced_rx,
        outcome_tx,
        validator.clone(),
        None,
    );
    let subnet = subnets
        .subnet_for_committee_at_slot(cluster.committee_id(), DUTY_SLOT)
        .unwrap();
    let topic =
        IdentTopic::new(subnets.router().topic_for_subnet_at_slot(subnet, DUTY_SLOT)).hash();
    let context = TopicContext::Validate {
        parsed: parse_topic(&topic).unwrap(),
    };
    let receive = |message: &SignedSSVMessage| {
        receiver
            .receive(
                PeerId::random(),
                MessageId::from(vec![message.operator_ids()[0].0 as u8]),
                Message {
                    source: None,
                    data: message.as_ssz_bytes(),
                    sequence_number: None,
                    topic: topic.clone(),
                },
                context.clone(),
            )
            .unwrap();
    };
    let collection_manager = collector.clone();
    let local_key = share(OWN_SIGNER).clone();
    let mut collection = tokio::spawn(async move {
        collection_manager
            .sign_and_collect(
                SignatureMetadata {
                    kind: PartialSignatureKind::ProposerPreferences,
                    role: Role::ProposerPreferences,
                    threshold: THRESHOLD,
                    slot: DUTY_SLOT,
                    committee_id: cluster.committee_id(),
                },
                SignatureRequester::SingleValidator {
                    pubkey: validator_x.public_key,
                },
                ValidatorSigningData {
                    root: SIGNING_ROOT,
                    index: VALIDATOR_X,
                    validator_pubkey: validator_x.public_key,
                    share: Some(local_key),
                },
            )
            .await
    });
    tokio::time::timeout(TEST_TIMEOUT, outbound_rx.recv())
        .await
        .unwrap()
        .unwrap();
    // This control share represents one contribution already available to the collector. With
    // the local share, either the ignored signer or the fresh signer would complete 3-of-4.
    collector
        .receive_partial_signatures(partial(CONTROL_SIGNER, share(CONTROL_SIGNER)))
        .unwrap();
    let ignored = signed_partial(
        IGNORED_SIGNER,
        share(IGNORED_SIGNER),
        &rsa_keys[0],
        validator_x.public_key,
    );

    // Act: the unchanged registry is complete, but schedule A assigns Y at X's preference slot.
    assert!(matches!(
        validator
            .validate(&ignored.as_ssz_bytes(), &context)
            .as_result(),
        Err(ValidationFailure::NoDuty)
    ));
    receive(&ignored);
    assert!(matches!(
        tokio::time::timeout(TEST_TIMEOUT, outcomes.recv())
            .await
            .unwrap()
            .unwrap()
            .action,
        MessageAcceptance::Ignore
    ));
    drain_processor(&processor).await;

    // Assert: drain runnable collector work before virtual time advances. This current-thread
    // runtime cannot complete its 3-of-4 collection from the ignored share. No wall-time sleep
    // is used to guess whether delivery has happened.
    tokio::time::pause();
    assert!(
        tokio::time::timeout(QUIESCENCE_TIMEOUT, &mut collection)
            .await
            .is_err(),
        "ignored share reached the collector"
    );
    tokio::time::resume();

    // Act: expose exact local evidence for X while the complete HTTP schedule still assigns Y.
    tracker
        .set_local_proposer_lookup(move |slot, pubkey| {
            slot == DUTY_SLOT && *pubkey == validator_x.public_key
        })
        .unwrap();
    assert_eq!(
        tracker.proposer_assignment_at_slot(DUTY_SLOT, &validator_x.public_key),
        DutyAssignment::NotAssigned
    );
    assert_eq!(
        tracker.proposer_assignment_at_slot(DUTY_SLOT, &validator_y.public_key),
        DutyAssignment::Assigned
    );
    let fresh = signed_partial(
        FRESH_SIGNER,
        share(FRESH_SIGNER),
        &rsa_keys[1],
        validator_x.public_key,
    );
    receive(&fresh);

    // Assert: a previously unseen signer passes the same receiver and completes actual BLS
    // reconstruction. An accepted outcome alone would not prove delivery to the collector.
    assert!(matches!(
        tokio::time::timeout(TEST_TIMEOUT, outcomes.recv())
            .await
            .unwrap()
            .unwrap()
            .action,
        MessageAcceptance::Accept
    ));
    let signature = tokio::time::timeout(TEST_TIMEOUT, collection)
        .await
        .expect("fresh share must reach collector")
        .unwrap()
        .unwrap();
    assert!(signature.verify(&master.public_key(), SIGNING_ROOT));
    server.abort();
}
