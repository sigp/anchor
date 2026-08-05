use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};

use bls::{INFINITY_PUBLIC_KEY, PublicKeyBytes, SecretKey, Signature};
use bls_lagrange::{KeyId, split_with_rng};
use database::{
    NetworkDatabase, PendingStateUpdates,
    test_utils::{TEST_NETWORK, commit_and_publish, generators},
};
use rand::{prelude::*, rngs::StdRng};
use ssv_types::{ENCRYPTED_KEY_LENGTH, Share, ValidatorIndex, ValidatorMetadata};
use tokio::sync::{Mutex, oneshot};
use types::Graffiti;

use super::*;

const TEST_RNG_SEED: u64 = 0xDEAD_BEEF_CAFE_0001;
const TOTAL_SHARES: u64 = 5;
const THRESHOLD: u64 = 3;
const SIGNING_ROOT: Hash256 = Hash256::repeat_byte(0xAB);
const WRONG_ROOT: Hash256 = Hash256::repeat_byte(0xCD);

static METRIC_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct TestKeys {
    master: SecretKey,
    shares: Vec<(OperatorId, SecretKey)>,
}

fn split_random_master() -> TestKeys {
    let rng = &mut StdRng::seed_from_u64(TEST_RNG_SEED);
    let master = SecretKey::random();
    let shares = split_with_rng(
        &master,
        THRESHOLD,
        (1..=TOTAL_SHARES).map(|id| KeyId::try_from(id).expect("non-zero key id")),
        rng,
    )
    .expect("split should succeed")
    .into_iter()
    .map(|(key_id, secret_key)| (OperatorId(u64::from(key_id)), secret_key))
    .collect();
    TestKeys { master, shares }
}

fn register_notifier(
    state: &mut SignatureCollectorState,
    validator_pubkey: PublicKeyBytes,
) -> oneshot::Receiver<Arc<Signature>> {
    let (notify, receiver) = oneshot::channel();
    assert!(
        state
            .register_request(notify, THRESHOLD, validator_pubkey)
            .is_continue()
    );
    receiver
}

fn add_valid_share(
    state: &mut SignatureCollectorState,
    operator_id: OperatorId,
    secret_key: &SecretKey,
) {
    state.add_partial_signature(operator_id, secret_key.sign(SIGNING_ROOT));
}

fn complete_ready_reconstruction(state: &mut SignatureCollectorState) {
    let signature = match state.try_reconstruct() {
        Ok(Some(signature)) => signature,
        Ok(None) => panic!("expected enough shares to reconstruct"),
        Err(_) => panic!("expected reconstructed signature to verify"),
    };
    state.complete_reconstruction(signature);
}

#[track_caller]
fn expect_master_verification_failure(state: &SignatureCollectorState) -> ReconstructionFailure {
    match state.try_reconstruct() {
        Err(failure @ ReconstructionFailure::MasterVerification) => failure,
        outcome => panic!("expected master signature verification to fail, got {outcome:?}"),
    }
}

#[track_caller]
fn expect_combination_failure(state: &SignatureCollectorState) -> ReconstructionFailure {
    match state.try_reconstruct() {
        Err(failure @ ReconstructionFailure::Combination(_)) => failure,
        outcome => panic!("expected signature combination to fail, got {outcome:?}"),
    }
}

async fn enter_fallback(
    state: &mut SignatureCollectorState,
    failure: ReconstructionFailure,
    database: Arc<NetworkDatabase>,
) -> ControlFlow<()> {
    handle_reconstruction_fallback(state, failure, &SharePubkeyLoader::new(database)).await
}

fn expect_signature(receiver: &mut oneshot::Receiver<Arc<Signature>>, expected: &Signature) {
    let signature = receiver
        .try_recv()
        .expect("notifier should receive a verified signature");
    assert_eq!(signature.as_ref(), expected);
}

fn fallback_count() -> u64 {
    metrics::RECONSTRUCTION_FALLBACKS_TOTAL
        .as_ref()
        .expect("fallback metric should register")
        .get()
}

fn corrupt_database(database: &NetworkDatabase, sql: &str) {
    database
        .connection()
        .expect("connection should open")
        .execute(sql, [])
        .expect("corruption statement should execute");
}

fn database_with_share_keys(
    keys: &TestKeys,
    validator_pubkey: PublicKeyBytes,
) -> Arc<NetworkDatabase> {
    let operators = keys
        .shares
        .iter()
        .map(|(operator_id, _)| generators::operator::with_id(operator_id.0))
        .collect::<Vec<_>>();
    let database = NetworkDatabase::new_in_memory(&operators[0].rsa_pubkey, TEST_NETWORK)
        .expect("in-memory database should open");
    let cluster = generators::cluster::with_operators(&operators);
    let validator = ValidatorMetadata {
        public_key: validator_pubkey,
        cluster_id: cluster.cluster_id,
        index: Some(ValidatorIndex(1)),
        graffiti: Graffiti::default(),
    };
    let shares = keys
        .shares
        .iter()
        .map(|(operator_id, secret_key)| Share {
            validator_pubkey,
            operator_id: *operator_id,
            cluster_id: cluster.cluster_id,
            share_pubkey: secret_key.public_key().compress(),
            encrypted_private_key: [0; ENCRYPTED_KEY_LENGTH],
        })
        .collect::<Vec<_>>();

    {
        let mut connection = database.connection().expect("connection should open");
        let transaction = connection.transaction().expect("transaction should start");
        let mut pending = PendingStateUpdates::default();
        for operator in &operators {
            database
                .insert_operator_tx(operator, &transaction, &mut pending)
                .expect("operator should be inserted");
        }
        database
            .insert_validator_tx(cluster, &validator, shares, &transaction, &mut pending)
            .expect("validator should be inserted");
        commit_and_publish(&database, transaction, pending);
    }

    Arc::new(database)
}

#[tokio::test]
async fn clean_quorum_is_verified_cached_and_does_not_enter_fallback() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let count_before = fallback_count();
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let expected = keys.master.sign(SIGNING_ROOT);
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);

    let mut first = register_notifier(&mut state, validator_pubkey);
    let mut second = register_notifier(&mut state, validator_pubkey);
    for (operator_id, secret_key) in &keys.shares[..(THRESHOLD - 1) as usize] {
        add_valid_share(&mut state, *operator_id, secret_key);
    }
    assert!(matches!(state.try_reconstruct(), Ok(None)));
    add_valid_share(
        &mut state,
        keys.shares[(THRESHOLD - 1) as usize].0,
        &keys.shares[(THRESHOLD - 1) as usize].1,
    );
    complete_ready_reconstruction(&mut state);
    expect_signature(&mut first, &expected);
    expect_signature(&mut second, &expected);

    let mut late = register_notifier(&mut state, validator_pubkey);
    expect_signature(&mut late, &expected);
    add_valid_share(
        &mut state,
        keys.shares[THRESHOLD as usize].0,
        &keys.shares[THRESHOLD as usize].1,
    );

    assert!(state.signature_share.is_empty());
    assert_eq!(fallback_count(), count_before);
}

#[tokio::test]
async fn wrong_root_share_is_pruned_then_a_fourth_honest_share_completes() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let count_before = fallback_count();
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let database = database_with_share_keys(&keys, validator_pubkey);
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);
    let mut receiver = register_notifier(&mut state, validator_pubkey);

    add_valid_share(&mut state, keys.shares[0].0, &keys.shares[0].1);
    add_valid_share(&mut state, keys.shares[1].0, &keys.shares[1].1);
    state.add_partial_signature(keys.shares[2].0, keys.shares[2].1.sign(WRONG_ROOT));
    let failure = expect_master_verification_failure(&state);

    assert!(
        enter_fallback(&mut state, failure, database)
            .await
            .is_continue()
    );
    assert_eq!(state.signature_share.len(), 2);
    assert!(!state.signature_share.contains_key(&keys.shares[2].0));

    add_valid_share(&mut state, keys.shares[3].0, &keys.shares[3].1);
    complete_ready_reconstruction(&mut state);
    expect_signature(&mut receiver, &keys.master.sign(SIGNING_ROOT));
    assert_eq!(fallback_count(), count_before + 1);
}

#[tokio::test]
async fn empty_share_is_pruned_and_same_operator_can_replace_it() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let count_before = fallback_count();
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let database = database_with_share_keys(&keys, validator_pubkey);
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);
    let mut receiver = register_notifier(&mut state, validator_pubkey);

    add_valid_share(&mut state, keys.shares[0].0, &keys.shares[0].1);
    add_valid_share(&mut state, keys.shares[1].0, &keys.shares[1].1);
    let invalid_operator = keys.shares[2].0;
    state.add_partial_signature(invalid_operator, Signature::empty());
    let failure = expect_combination_failure(&state);

    assert!(
        enter_fallback(&mut state, failure, database)
            .await
            .is_continue()
    );
    assert!(!state.signature_share.contains_key(&invalid_operator));

    add_valid_share(&mut state, invalid_operator, &keys.shares[2].1);
    complete_ready_reconstruction(&mut state);
    expect_signature(&mut receiver, &keys.master.sign(SIGNING_ROOT));
    assert_eq!(fallback_count(), count_before + 1);
}

#[tokio::test]
async fn buffered_bad_shares_are_pruned_and_honest_quorum_retries_immediately() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let count_before = fallback_count();
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let database = database_with_share_keys(&keys, validator_pubkey);
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);

    for (operator_id, secret_key) in &keys.shares[..THRESHOLD as usize] {
        add_valid_share(&mut state, *operator_id, secret_key);
    }
    state.add_partial_signature(keys.shares[3].0, keys.shares[3].1.sign(WRONG_ROOT));
    state.add_partial_signature(keys.shares[4].0, Signature::empty());

    let (notify, mut receiver) = oneshot::channel();
    assert!(
        state
            .register_request(notify, THRESHOLD, validator_pubkey)
            .is_continue()
    );
    let failure = expect_combination_failure(&state);
    assert!(
        enter_fallback(&mut state, failure, database)
            .await
            .is_continue()
    );

    expect_signature(&mut receiver, &keys.master.sign(SIGNING_ROOT));
    assert!(state.signature_share.is_empty());
    assert_eq!(fallback_count(), count_before + 1);
}

#[test]
fn missing_and_undecompressible_share_keys_remove_only_affected_shares() {
    let keys = split_random_master();
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);
    for (operator_id, secret_key) in &keys.shares[..THRESHOLD as usize] {
        add_valid_share(&mut state, *operator_id, secret_key);
    }
    let undecompressible = PublicKeyBytes::deserialize(&INFINITY_PUBLIC_KEY)
        .expect("infinity public key should have the correct length");
    let share_pubkeys = HashMap::from([
        (keys.shares[0].0, keys.shares[0].1.public_key().compress()),
        (keys.shares[2].0, undecompressible),
    ]);

    assert_eq!(
        state.remove_invalid_shares(&share_pubkeys),
        vec![keys.shares[1].0, keys.shares[2].0]
    );
    assert_eq!(
        state.signature_share.keys().copied().collect::<Vec<_>>(),
        vec![keys.shares[0].0]
    );
}

#[tokio::test]
async fn individually_valid_shares_with_wrong_master_key_are_fatal() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let count_before = fallback_count();
    let keys = split_random_master();
    let other_keys = split_random_master();
    let wrong_validator_pubkey = other_keys.master.public_key().compress();
    let database = database_with_share_keys(&keys, wrong_validator_pubkey);
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);
    let _receiver = register_notifier(&mut state, wrong_validator_pubkey);

    add_valid_share(&mut state, keys.shares[0].0, &keys.shares[0].1);
    add_valid_share(&mut state, keys.shares[1].0, &keys.shares[1].1);
    state.add_partial_signature(keys.shares[2].0, keys.shares[2].1.sign(SIGNING_ROOT));
    let failure = expect_master_verification_failure(&state);

    assert!(
        enter_fallback(&mut state, failure, database)
            .await
            .is_break()
    );
    assert!(state.full_signature.is_none());
    assert_eq!(fallback_count(), count_before + 1);
}

#[test]
fn malformed_and_conflicting_registrations_exit_before_notifying() {
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let malformed = PublicKeyBytes::deserialize(&INFINITY_PUBLIC_KEY)
        .expect("infinity public key should have the correct length");

    let mut malformed_state = SignatureCollectorState::new(SIGNING_ROOT);
    let (notify, _) = oneshot::channel();
    assert!(
        malformed_state
            .register_request(notify, THRESHOLD, malformed)
            .is_break()
    );

    let mut threshold_state = SignatureCollectorState::new(SIGNING_ROOT);
    let _receiver = register_notifier(&mut threshold_state, validator_pubkey);
    let (notify, _) = oneshot::channel();
    assert!(
        threshold_state
            .register_request(notify, THRESHOLD + 1, validator_pubkey)
            .is_break()
    );

    let mut cached_state = SignatureCollectorState::new(SIGNING_ROOT);
    let mut receiver = register_notifier(&mut cached_state, validator_pubkey);
    for (operator_id, secret_key) in &keys.shares[..THRESHOLD as usize] {
        add_valid_share(&mut cached_state, *operator_id, secret_key);
    }
    complete_ready_reconstruction(&mut cached_state);
    expect_signature(&mut receiver, &keys.master.sign(SIGNING_ROOT));
    let (notify, _) = oneshot::channel();
    let conflicting_pubkey = split_random_master().master.public_key().compress();
    assert!(
        cached_state
            .register_request(notify, THRESHOLD, conflicting_pubkey)
            .is_break()
    );
}

#[tokio::test]
async fn empty_malformed_and_failed_database_lookups_are_fatal() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();

    let empty_database = Arc::new(
        NetworkDatabase::new_in_memory(&generators::pubkey::random_rsa(), TEST_NETWORK)
            .expect("empty database should open"),
    );
    let mut empty_state = SignatureCollectorState::new(SIGNING_ROOT);
    let mut empty_receiver = register_notifier(&mut empty_state, validator_pubkey);
    add_valid_share(&mut empty_state, keys.shares[0].0, &keys.shares[0].1);
    add_valid_share(&mut empty_state, keys.shares[1].0, &keys.shares[1].1);
    empty_state.add_partial_signature(keys.shares[2].0, keys.shares[2].1.sign(WRONG_ROOT));
    let failure = expect_master_verification_failure(&empty_state);
    assert!(
        enter_fallback(&mut empty_state, failure, empty_database)
            .await
            .is_break()
    );
    assert!(empty_state.full_signature.is_none());
    drop(empty_state);
    assert!(matches!(
        empty_receiver.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));

    let malformed_database = database_with_share_keys(&keys, validator_pubkey);
    corrupt_database(
        &malformed_database,
        "UPDATE shares SET share_pubkey = 'not-a-public-key'",
    );
    assert!(
        SharePubkeyLoader::new(malformed_database)
            .fetch(validator_pubkey)
            .await
            .is_none()
    );

    let failed_database = database_with_share_keys(&keys, validator_pubkey);
    corrupt_database(&failed_database, "DROP TABLE shares");
    assert!(
        SharePubkeyLoader::new(failed_database)
            .fetch(validator_pubkey)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn collector_loop_database_failure_closes_notifier_without_caching() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let count_before = fallback_count();
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let database = database_with_share_keys(&keys, validator_pubkey);
    corrupt_database(&database, "DROP TABLE shares");

    let (tx, rx) = mpsc::unbounded_channel::<CollectorMessage<()>>();
    let (_lifetime_guard, lifetime_end) = oneshot::channel();
    let collector = tokio::spawn(signature_collector(
        rx,
        SIGNING_ROOT,
        SharePubkeyLoader::new(database),
        lifetime_end,
    ));
    let send = |kind| {
        tx.send(CollectorMessage {
            kind,
            _drop_on_finish: (),
        })
        .expect("collector should accept messages while running");
    };

    let (notify, result_rx) = oneshot::channel();
    send(CollectorMessageKind::RegisterNotifier {
        notify,
        threshold: THRESHOLD,
        validator_pubkey,
    });
    for (operator_id, secret_key) in &keys.shares[..(THRESHOLD - 1) as usize] {
        send(CollectorMessageKind::PartialSignature {
            operator_id: *operator_id,
            signature: Box::new(secret_key.sign(SIGNING_ROOT)),
        });
    }
    send(CollectorMessageKind::PartialSignature {
        operator_id: keys.shares[(THRESHOLD - 1) as usize].0,
        signature: Box::new(keys.shares[(THRESHOLD - 1) as usize].1.sign(WRONG_ROOT)),
    });

    let result = tokio::time::timeout(Duration::from_secs(5), result_rx)
        .await
        .expect("collector should close the notifier promptly");
    assert!(
        result.is_err(),
        "database fallback failure must close the notifier without returning a signature"
    );
    tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .expect("collector task should terminate promptly")
        .expect("collector task should not panic");
    assert_eq!(fallback_count(), count_before + 1);
}

#[tokio::test(flavor = "current_thread")]
async fn guard_dropped_before_first_poll_exits_without_processing() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let count_before = fallback_count();
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let loader = SharePubkeyLoader::new(database_with_share_keys(&keys, validator_pubkey));
    let (tx, rx) = mpsc::unbounded_channel::<CollectorMessage<()>>();
    let send = |kind| {
        tx.send(CollectorMessage {
            kind,
            _drop_on_finish: (),
        })
        .expect("collector message should queue");
    };

    let (notify, result_rx) = oneshot::channel();
    send(CollectorMessageKind::RegisterNotifier {
        notify,
        threshold: THRESHOLD,
        validator_pubkey,
    });
    for (operator_id, secret_key) in &keys.shares[..THRESHOLD as usize] {
        send(CollectorMessageKind::PartialSignature {
            operator_id: *operator_id,
            signature: Box::new(secret_key.sign(SIGNING_ROOT)),
        });
    }

    let (lifetime_guard, lifetime_end) = oneshot::channel();
    drop(lifetime_guard);
    let collector = tokio::spawn(signature_collector(rx, SIGNING_ROOT, loader, lifetime_end));

    let result = tokio::time::timeout(Duration::from_secs(5), result_rx)
        .await
        .expect("collector should close the notifier promptly");
    assert!(
        result.is_err(),
        "an expired collector must not process an already-queued quorum"
    );
    tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .expect("expired collector task should terminate promptly")
        .expect("expired collector task should not panic");
    assert_eq!(fallback_count(), count_before);
}

#[tokio::test(flavor = "current_thread")]
async fn collector_map_removal_cancels_fallback_wait() {
    let _metric_guard = METRIC_TEST_LOCK.lock().await;
    let count_before = fallback_count();
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let loader = SharePubkeyLoader::new(database_with_share_keys(&keys, validator_pubkey));
    let held_permit = Arc::clone(&loader.semaphore)
        .acquire_owned()
        .await
        .expect("fallback semaphore should be open");

    let (lifetime_guard, lifetime_end) = oneshot::channel();
    let map = DashMap::new();
    let key = (SIGNING_ROOT, ValidatorIndex(1));
    let (entry_tx, _entry_rx) = mpsc::unbounded_channel::<CollectorMessage>();
    map.insert(
        key,
        SignatureCollector {
            _lifetime_guard: lifetime_guard,
            sender: entry_tx,
            for_slot: Slot::new(0),
        },
    );

    let (tx, rx) = mpsc::unbounded_channel::<CollectorMessage<()>>();
    let collector = tokio::spawn(signature_collector(
        rx,
        SIGNING_ROOT,
        loader.clone(),
        lifetime_end,
    ));
    let send = |kind| {
        tx.send(CollectorMessage {
            kind,
            _drop_on_finish: (),
        })
        .expect("collector should accept messages while running");
    };

    let (notify, mut result_rx) = oneshot::channel();
    send(CollectorMessageKind::RegisterNotifier {
        notify,
        threshold: THRESHOLD,
        validator_pubkey,
    });
    for (operator_id, secret_key) in &keys.shares[..(THRESHOLD - 1) as usize] {
        send(CollectorMessageKind::PartialSignature {
            operator_id: *operator_id,
            signature: Box::new(secret_key.sign(SIGNING_ROOT)),
        });
    }
    send(CollectorMessageKind::PartialSignature {
        operator_id: keys.shares[(THRESHOLD - 1) as usize].0,
        signature: Box::new(keys.shares[(THRESHOLD - 1) as usize].1.sign(WRONG_ROOT)),
    });

    tokio::time::timeout(Duration::from_secs(5), async {
        while fallback_count() == count_before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("collector should enter reconstruction fallback");
    assert_eq!(fallback_count(), count_before + 1);
    assert!(!collector.is_finished());
    assert!(matches!(
        result_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(loader.semaphore.available_permits(), 0);

    drop(map.remove(&key).expect("collector map entry should exist"));

    let result = tokio::time::timeout(Duration::from_secs(5), result_rx)
        .await
        .expect("collector should close the notifier after map removal");
    assert!(
        result.is_err(),
        "collector cancellation must not return a signature"
    );
    tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .expect("collector task should terminate after map removal")
        .expect("collector task should not panic");
    assert_eq!(loader.semaphore.available_permits(), 0);

    drop(held_permit);
    let _permit = loader
        .semaphore
        .try_acquire()
        .expect("cancelled fallback should not retain a semaphore permit");
}
