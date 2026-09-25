use std::{
    collections::HashMap,
    sync::{
        Arc, Condvar, LazyLock, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
        mpsc as std_mpsc,
    },
    time::Duration,
};

use bls::{INFINITY_PUBLIC_KEY, PublicKeyBytes, SecretKey, Signature};
use bls_lagrange::{KeyId, split_with_rng};
use database::{
    NetworkDatabase, OwnOperatorId, PendingStateUpdates,
    test_utils::{TEST_NETWORK, commit_and_publish, generators},
};
use fork::{Fork, ForkSchedule};
use message_sender::{Error as MessageSenderError, MessageSender};
use processor::{Config as ProcessorConfig, spawn as spawn_processor};
use rand::{prelude::*, rngs::StdRng};
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    ENCRYPTED_KEY_LENGTH, Share, ValidatorIndex, ValidatorMetadata, VariableList,
    domain_type::DomainType, message::SignedSSVMessage,
};
use ssz::Decode;
use task_executor::test_utils::TestRuntime;
use tokio::sync::{Mutex, oneshot};
use types::{Graffiti, SyncSubnetId};

use super::*;

const TEST_RNG_SEED: u64 = 0xDEAD_BEEF_CAFE_0001;
const TOTAL_SHARES: u64 = 5;
const THRESHOLD: u64 = 3;
const SIGNING_ROOT: Hash256 = Hash256::repeat_byte(0xAB);
const WRONG_ROOT: Hash256 = Hash256::repeat_byte(0xCD);
const TEST_OPERATOR_ID: OperatorId = OperatorId(1);
const BATCH_TEST_SLOT: Slot = Slot::new(64);
const SLOTS_PER_EPOCH: u64 = 32;

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
            .register_request(Some(notify), THRESHOLD, validator_pubkey, None)
            .is_continue()
    );
    receiver
}

fn add_valid_share(
    state: &mut SignatureCollectorState,
    operator_id: OperatorId,
    secret_key: &SecretKey,
) {
    state.add_partial_signature(operator_id, secret_key.sign(SIGNING_ROOT), None);
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

struct RecordingMessageSender {
    messages: StdMutex<Vec<UnsignedSSVMessage>>,
    attempts: AtomicUsize,
    fail_first_attempts: usize,
}

impl RecordingMessageSender {
    fn new(fail_first_attempts: usize) -> Self {
        Self {
            messages: StdMutex::new(vec![]),
            attempts: AtomicUsize::new(0),
            fail_first_attempts,
        }
    }

    fn messages(&self) -> Vec<UnsignedSSVMessage> {
        self.messages
            .lock()
            .expect("message capture lock should not be poisoned")
            .clone()
    }

    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

impl MessageSender for RecordingMessageSender {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        _committee_id: CommitteeId,
        _additional_message_callback: Option<Box<dyn FnOnce(&SignedSSVMessage) + Send + 'static>>,
    ) -> Result<(), MessageSenderError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.fail_first_attempts {
            return Err(MessageSenderError::NetworkQueueClosed);
        }
        self.messages
            .lock()
            .expect("message capture lock should not be poisoned")
            .push(message);
        Ok(())
    }

    fn send(
        &self,
        _message: SignedSSVMessage,
        _committee_id: CommitteeId,
    ) -> Result<(), MessageSenderError> {
        Ok(())
    }
}

struct BlockingMessageSender {
    messages: StdMutex<Vec<UnsignedSSVMessage>>,
    attempts: AtomicUsize,
    first_attempt_entered: StdMutex<Option<std_mpsc::SyncSender<()>>>,
    release_first_attempt: (StdMutex<bool>, Condvar),
}

impl BlockingMessageSender {
    fn new() -> (Arc<Self>, std_mpsc::Receiver<()>) {
        let (entered_tx, entered_rx) = std_mpsc::sync_channel(1);
        (
            Arc::new(Self {
                messages: StdMutex::new(vec![]),
                attempts: AtomicUsize::new(0),
                first_attempt_entered: StdMutex::new(Some(entered_tx)),
                release_first_attempt: (StdMutex::new(false), Condvar::new()),
            }),
            entered_rx,
        )
    }

    fn release(&self) {
        let (released, condvar) = &self.release_first_attempt;
        *released
            .lock()
            .expect("release lock should not be poisoned") = true;
        condvar.notify_all();
    }

    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

impl MessageSender for BlockingMessageSender {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        _committee_id: CommitteeId,
        _additional_message_callback: Option<Box<dyn FnOnce(&SignedSSVMessage) + Send + 'static>>,
    ) -> Result<(), MessageSenderError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            if let Some(entered) = self
                .first_attempt_entered
                .lock()
                .expect("entry notification lock should not be poisoned")
                .take()
            {
                entered
                    .send(())
                    .expect("test should wait for the blocked sender");
            }
            let (released, condvar) = &self.release_first_attempt;
            let _released = condvar
                .wait_while(
                    released
                        .lock()
                        .expect("release lock should not be poisoned"),
                    |released| !*released,
                )
                .expect("release lock should not be poisoned");
        }
        self.messages
            .lock()
            .expect("message capture lock should not be poisoned")
            .push(message);
        Ok(())
    }

    fn send(
        &self,
        _message: SignedSSVMessage,
        _committee_id: CommitteeId,
    ) -> Result<(), MessageSenderError> {
        Ok(())
    }
}

struct BatchScenario {
    manager: Arc<SignatureCollectorManager<ManualSlotClock>>,
    metadata: SignatureMetadata,
    validator_master: SecretKey,
    validator_key: SecretKey,
    remote_shares: Vec<(OperatorId, SecretKey)>,
    validator_pubkey: PublicKeyBytes,
    _runtime: TestRuntime,
}

impl BatchScenario {
    fn new(message_sender: Arc<dyn MessageSender>) -> Self {
        Self::new_with_max_workers(message_sender, 4)
    }

    fn new_with_max_workers(message_sender: Arc<dyn MessageSender>, max_workers: usize) -> Self {
        let fork_schedule = Arc::new(ForkSchedule::new(Fork::Alan, DomainType::default(), "test"));
        Self::new_with_max_workers_and_fork_schedule(message_sender, max_workers, fork_schedule)
    }

    fn new_with_max_workers_and_fork_schedule(
        message_sender: Arc<dyn MessageSender>,
        max_workers: usize,
        fork_schedule: Arc<ForkSchedule>,
    ) -> Self {
        let runtime = TestRuntime::default();
        let processor = spawn_processor(
            ProcessorConfig {
                max_workers,
                ..ProcessorConfig::default()
            },
            runtime.task_executor.clone(),
        );
        let rsa_pubkey = generators::pubkey::random_rsa();
        let database = Arc::new(
            NetworkDatabase::new_in_memory(&rsa_pubkey, TEST_NETWORK)
                .expect("in-memory database should open"),
        );
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            Duration::from_secs(0),
            Duration::from_secs(12),
        );
        slot_clock.set_slot(BATCH_TEST_SLOT.as_u64());
        let manager = SignatureCollectorManager::new(
            processor,
            OwnOperatorId::Known(TEST_OPERATOR_ID),
            database,
            fork_schedule,
            SLOTS_PER_EPOCH,
            message_sender,
            slot_clock,
        )
        .expect("manager should be created");
        let keys = split_random_master();
        let validator_pubkey = keys.master.public_key().compress();
        let validator_key = keys.shares[0].1.clone();
        let remote_shares = keys.shares[1..THRESHOLD as usize].to_vec();

        Self {
            manager,
            metadata: SignatureMetadata {
                kind: PartialSignatureKind::ContributionProofs,
                role: Role::SyncCommittee,
                threshold: THRESHOLD,
                slot: BATCH_TEST_SLOT,
                committee_id: CommitteeId::default(),
            },
            validator_master: keys.master,
            validator_key,
            remote_shares,
            validator_pubkey,
            _runtime: runtime,
        }
    }

    fn seed_remote_shares(&self, signing_root: Hash256, validator_index: ValidatorIndex) {
        for (operator_id, share) in &self.remote_shares {
            self.manager
                .receive_partial_signature(
                    PartialSignatureMessage {
                        partial_signature: share.sign(signing_root),
                        signing_root,
                        signer: *operator_id,
                        validator_index,
                    },
                    self.metadata.slot,
                )
                .expect("remote partial signature should enter the processor");
        }
    }
}

fn descriptor(entries: &[(u64, Hash256, usize)]) -> Vec<SyncCommitteeBatchEntry> {
    entries
        .iter()
        .map(
            |(subnet_id, signing_root, multiplicity)| SyncCommitteeBatchEntry {
                subnet_id: SyncSubnetId::new(*subnet_id),
                signing_root: *signing_root,
                multiplicity: *multiplicity,
            },
        )
        .collect()
}

fn batch_key(
    metadata: &SignatureMetadata,
    pubkey: PublicKeyBytes,
) -> (SingleValidatorBatchPhase, Slot, PublicKeyBytes) {
    (
        SingleValidatorBatchPhase::from_kind(metadata.kind)
            .expect("batch test metadata should use a supported phase"),
        metadata.slot,
        pubkey,
    )
}

fn process_batch(
    manager: &SignatureCollectorManager<ManualSlotClock>,
    metadata: &SignatureMetadata,
    pubkey: PublicKeyBytes,
    validator_key: &SecretKey,
    validator_index: ValidatorIndex,
    subnet_id: SyncSubnetId,
    descriptor: Vec<SyncCommitteeBatchEntry>,
) -> Vec<PartialSignatureMessage> {
    let signing_root = descriptor
        .iter()
        .find(|entry| entry.subnet_id == subnet_id)
        .expect("callback subnet should be in its descriptor")
        .signing_root;
    process_batch_for_root(
        manager,
        metadata,
        pubkey,
        validator_key,
        validator_index,
        subnet_id,
        signing_root,
        descriptor,
    )
}

#[expect(clippy::too_many_arguments)]
fn process_batch_for_root(
    manager: &SignatureCollectorManager<ManualSlotClock>,
    metadata: &SignatureMetadata,
    pubkey: PublicKeyBytes,
    validator_key: &SecretKey,
    validator_index: ValidatorIndex,
    subnet_id: SyncSubnetId,
    signing_root: Hash256,
    descriptor: Vec<SyncCommitteeBatchEntry>,
) -> Vec<PartialSignatureMessage> {
    let signing_data = ValidatorSigningData {
        root: signing_root,
        index: validator_index,
        validator_pubkey: pubkey,
        share: Some(validator_key.clone()),
    };
    let current_message = PartialSignatureMessage {
        partial_signature: validator_key.sign(signing_root),
        signing_root,
        signer: TEST_OPERATOR_ID,
        validator_index,
    };
    manager.process_single_validator_batch(
        metadata,
        pubkey,
        subnet_id,
        descriptor,
        &signing_data,
        current_message,
    )
}

async fn sign_and_collect_batch(
    scenario: &BatchScenario,
    metadata: SignatureMetadata,
    validator_index: ValidatorIndex,
    subnet_id: SyncSubnetId,
    descriptor: Vec<SyncCommitteeBatchEntry>,
) -> Arc<Signature> {
    let signing_root = descriptor
        .iter()
        .find(|entry| entry.subnet_id == subnet_id)
        .expect("callback subnet should be in its descriptor")
        .signing_root;
    scenario
        .manager
        .sign_and_collect(
            metadata,
            SignatureRequester::SingleValidatorBatch {
                pubkey: scenario.validator_pubkey,
                subnet_id,
                descriptor,
            },
            ValidatorSigningData {
                root: signing_root,
                index: validator_index,
                validator_pubkey: scenario.validator_pubkey,
                share: Some(scenario.validator_key.clone()),
            },
        )
        .await
        .expect("batch signature should be collected")
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition should become true promptly");
}

async fn drain_batch_processor_work(manager: &SignatureCollectorManager<ManualSlotClock>) {
    let (urgent_done_tx, urgent_done_rx) = oneshot::channel();
    manager
        .processor
        .urgent_consensus
        .send_blocking(
            move || {
                let _ = urgent_done_tx.send(());
            },
            "batch_test_urgent_barrier",
        )
        .expect("urgent barrier should enter the processor");
    tokio::time::timeout(Duration::from_secs(5), urgent_done_rx)
        .await
        .expect("urgent barrier should run promptly")
        .expect("urgent barrier should signal completion");

    let (permitless_done_tx, permitless_done_rx) = oneshot::channel();
    manager
        .processor
        .permitless
        .send_immediate(
            move |_drop_on_finish| {
                let _ = permitless_done_tx.send(());
            },
            "batch_test_permitless_barrier",
        )
        .expect("permitless barrier should enter the processor");
    tokio::time::timeout(Duration::from_secs(5), permitless_done_rx)
        .await
        .expect("permitless barrier should run promptly")
        .expect("permitless barrier should signal completion");
}

async fn register_manager_notifier(
    manager: &Arc<SignatureCollectorManager<ManualSlotClock>>,
    signing_root: Hash256,
    validator_index: ValidatorIndex,
    slot: Slot,
    validator_pubkey: PublicKeyBytes,
) -> Arc<Signature> {
    let (notify, result_rx) = oneshot::channel();
    let manager_clone = Arc::clone(manager);
    manager
        .processor
        .permitless
        .send_immediate(
            move |drop_on_finish| {
                let sender = manager_clone.get_or_spawn(signing_root, validator_index, slot);
                sender
                    .send(CollectorMessage {
                        kind: CollectorMessageKind::RegisterNotifier {
                            notify: Some(notify),
                            required_companion: None,
                            threshold: THRESHOLD,
                            validator_pubkey,
                        },
                        _drop_on_finish: drop_on_finish,
                    })
                    .expect("collector should accept a late notifier");
            },
            COLLECTOR_MESSAGE_NAME,
        )
        .expect("late notifier should enter the processor");

    tokio::time::timeout(Duration::from_secs(5), result_rx)
        .await
        .expect("late notifier should resolve promptly")
        .expect("collector should remain active")
}

#[test]
fn single_validator_batch_envelope_cases() {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    let root_0 = Hash256::repeat_byte(0x10);
    let root_1 = Hash256::repeat_byte(0x11);
    let cases = [
        (
            descriptor(&[(0, root_0, 1)]),
            0,
            vec![root_0],
            vec![root_0],
            PartialSignatureKind::ContributionProofs,
            "one position",
        ),
        (
            descriptor(&[(0, root_0, 2)]),
            0,
            vec![root_0, root_0],
            vec![root_0],
            PartialSignatureKind::ContributionProofs,
            "same-subnet multiplicity",
        ),
        (
            descriptor(&[(0, root_0, 2), (1, root_1, 1)]),
            1,
            vec![root_0, root_0, root_1],
            vec![root_1, root_0],
            PartialSignatureKind::ContributionProofs,
            "multiple subnets",
        ),
        (
            descriptor(&[(0, root_0, 2), (1, root_1, 1)]),
            0,
            vec![root_0, root_0, root_1],
            vec![root_0, root_1],
            PartialSignatureKind::PostConsensus,
            "post-consensus decided root multiset",
        ),
    ];

    for (
        case_index,
        (descriptor, callback_subnet, expected_wire_roots, expected_injection_roots, kind, label),
    ) in cases.into_iter().enumerate()
    {
        let mut metadata = scenario.metadata.clone();
        metadata.slot += case_index as u64;
        metadata.kind = kind;
        let unique_messages = process_batch(
            &scenario.manager,
            &metadata,
            scenario.validator_pubkey,
            &scenario.validator_key,
            ValidatorIndex(42),
            SyncSubnetId::new(callback_subnet),
            descriptor.clone(),
        );
        assert_eq!(unique_messages.len(), descriptor.len(), "{label}");
        assert_eq!(
            unique_messages
                .iter()
                .map(|message| message.signing_root)
                .collect::<Vec<_>>(),
            expected_injection_roots,
            "local injection must prioritize the current root"
        );

        let sent = sender.messages();
        let unsigned = &sent[case_index];
        assert!(unsigned.full_data.is_empty(), "{label}");
        assert_eq!(
            unsigned.ssv_message.msg_id(),
            &MessageId::new(
                &DomainType::default(),
                Role::SyncCommittee,
                &DutyExecutor::Validator(scenario.validator_pubkey),
            ),
            "{label}"
        );
        let messages = PartialSignatureMessages::from_ssz_bytes(unsigned.ssv_message.data())
            .expect("captured batch should decode");
        assert_eq!(messages.kind, kind, "{label}");
        assert_eq!(messages.slot, metadata.slot);
        assert_eq!(
            messages
                .messages
                .iter()
                .map(|message| message.signing_root)
                .collect::<Vec<_>>(),
            expected_wire_roots,
            "{label}"
        );
        assert!(messages.messages.iter().all(|message| {
            message.signer == TEST_OPERATOR_ID && message.validator_index == ValidatorIndex(42)
        }));
        for (message, signing_root) in messages.messages.iter().zip(&expected_wire_roots) {
            assert_eq!(
                message.partial_signature,
                scenario.validator_key.sign(*signing_root),
                "{label}"
            );
        }
    }
    assert_eq!(sender.attempts(), 4);
}

#[test]
fn post_consensus_uses_exact_roots_for_same_subnet_callbacks() {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = PartialSignatureKind::PostConsensus;
    let first_root = Hash256::repeat_byte(0x18);
    let callback_root = Hash256::repeat_byte(0x19);
    let descriptor = descriptor(&[(1, first_root, 1), (1, callback_root, 1)]);

    let injections = process_batch_for_root(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(1),
        callback_root,
        descriptor,
    );

    assert_eq!(
        injections
            .iter()
            .map(|message| message.signing_root)
            .collect::<Vec<_>>(),
        vec![callback_root, first_root]
    );
    let sent = sender.messages();
    assert_eq!(sent.len(), 1);
    let messages = PartialSignatureMessages::from_ssz_bytes(sent[0].ssv_message.data())
        .expect("post-consensus batch should decode");
    assert_eq!(messages.kind, PartialSignatureKind::PostConsensus);
    assert_eq!(messages.messages.len(), 2);
    assert_eq!(messages.messages[0].signing_root, first_root);
    assert_eq!(messages.messages[1].signing_root, callback_root);
    assert_eq!(
        messages.messages[0].partial_signature,
        scenario.validator_key.sign(first_root)
    );
    assert_eq!(
        messages.messages[1].partial_signature,
        scenario.validator_key.sign(callback_root)
    );
}

#[test]
fn post_consensus_injects_one_message_for_one_root_on_multiple_subnets() {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = PartialSignatureKind::PostConsensus;
    let signing_root = Hash256::repeat_byte(0x1A);

    let injections = process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor(&[(0, signing_root, 1), (1, signing_root, 1)]),
    );

    assert_eq!(injections.len(), 1);
    assert_eq!(injections[0].signing_root, signing_root);
    let sent = sender.messages();
    let messages = PartialSignatureMessages::from_ssz_bytes(sent[0].ssv_message.data())
        .expect("post-consensus batch should decode");
    assert_eq!(messages.kind, PartialSignatureKind::PostConsensus);
    assert_eq!(messages.messages.len(), 2);
    assert!(
        messages
            .messages
            .iter()
            .all(|message| message.signing_root == signing_root)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn post_consensus_impostor_sends_empty_batch_without_sibling_injection() {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario =
        BatchScenario::new_with_max_workers(Arc::clone(&sender) as Arc<dyn MessageSender>, 1);
    scenario.metadata.kind = PartialSignatureKind::PostConsensus;
    let validator_index = ValidatorIndex(42);
    let current_root = Hash256::repeat_byte(0x1B);
    let sibling_root = Hash256::repeat_byte(0x1C);
    let collector_key = (current_root, validator_index);
    let sibling_collector_key = (sibling_root, validator_index);
    let manager = Arc::clone(&scenario.manager);
    let metadata = scenario.metadata.clone();
    let validator_pubkey = scenario.validator_pubkey;

    let task = tokio::spawn(async move {
        manager
            .sign_and_collect(
                metadata,
                SignatureRequester::SingleValidatorBatch {
                    pubkey: validator_pubkey,
                    subnet_id: SyncSubnetId::new(0),
                    descriptor: descriptor(&[(0, current_root, 1), (1, sibling_root, 1)]),
                },
                ValidatorSigningData {
                    root: current_root,
                    index: validator_index,
                    validator_pubkey,
                    share: None,
                },
            )
            .await
    });

    wait_until(|| sender.attempts() == 1).await;
    drain_batch_processor_work(&scenario.manager).await;

    let sent = sender.messages();
    let messages = PartialSignatureMessages::from_ssz_bytes(sent[0].ssv_message.data())
        .expect("impostor post-consensus batch should decode");
    assert_eq!(messages.kind, PartialSignatureKind::PostConsensus);
    assert_eq!(messages.slot, scenario.metadata.slot);
    assert_eq!(messages.messages.len(), 2);
    assert_eq!(
        messages
            .messages
            .iter()
            .map(|message| message.signing_root)
            .collect::<Vec<_>>(),
        vec![current_root, sibling_root]
    );
    assert!(
        messages
            .messages
            .iter()
            .all(|message| message.partial_signature == Signature::empty()
                && message.signer == TEST_OPERATOR_ID
                && message.validator_index == validator_index)
    );
    assert!(
        scenario
            .manager
            .signature_collectors
            .contains_key(&collector_key)
    );
    assert!(
        !scenario
            .manager
            .signature_collectors
            .contains_key(&sibling_collector_key),
        "impostor mode must not inject an empty sibling share"
    );

    task.abort();
    let _ = task.await;
}

async fn run_first_callback_injection_and_reinjection(kind: PartialSignatureKind) {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = kind;
    let validator_index = ValidatorIndex(42);
    let root_0 = Hash256::repeat_byte(0x20);
    let root_1 = Hash256::repeat_byte(0x21);
    let descriptor = descriptor(&[(0, root_0, 2), (1, root_1, 1)]);
    scenario.seed_remote_shares(root_0, validator_index);
    scenario.seed_remote_shares(root_1, validator_index);

    let signature = sign_and_collect_batch(
        &scenario,
        scenario.metadata.clone(),
        validator_index,
        SyncSubnetId::new(0),
        descriptor.clone(),
    )
    .await;
    assert_eq!(signature.as_ref(), &scenario.validator_master.sign(root_0));
    assert_eq!(sender.attempts(), 1);

    wait_until(|| {
        scenario
            .manager
            .signature_collectors
            .contains_key(&(root_1, validator_index))
    })
    .await;
    let late_signature = register_manager_notifier(
        &scenario.manager,
        root_1,
        validator_index,
        scenario.metadata.slot,
        scenario.validator_pubkey,
    )
    .await;
    assert_eq!(
        late_signature.as_ref(),
        &scenario.validator_master.sign(root_1),
        "a quorum buffered before registration must resolve on the late notifier"
    );

    drop(
        scenario
            .manager
            .signature_collectors
            .remove(&(root_1, validator_index))
            .expect("sibling collector should exist"),
    );
    scenario.seed_remote_shares(root_1, validator_index);
    let reinjected = sign_and_collect_batch(
        &scenario,
        scenario.metadata.clone(),
        validator_index,
        SyncSubnetId::new(1),
        descriptor,
    )
    .await;
    assert_eq!(reinjected.as_ref(), &scenario.validator_master.sign(root_1));
    assert_eq!(
        sender.attempts(),
        1,
        "an admitted sibling must reinject its current root without sending again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn first_callback_injects_all_roots_and_admitted_sibling_reinjects() {
    run_first_callback_injection_and_reinjection(PartialSignatureKind::ContributionProofs).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn post_consensus_first_callback_injects_all_roots_and_admitted_sibling_reinjects() {
    run_first_callback_injection_and_reinjection(PartialSignatureKind::PostConsensus).await;
}

fn run_failed_admission_and_sibling_retry(kind: PartialSignatureKind) {
    let sender = Arc::new(RecordingMessageSender::new(1));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = kind;
    let root_0 = Hash256::repeat_byte(0x30);
    let root_1 = Hash256::repeat_byte(0x31);
    let descriptor = descriptor(&[(0, root_0, 1), (1, root_1, 1)]);

    let first_injections = process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    assert_eq!(first_injections.len(), 2);
    assert_eq!(sender.attempts(), 1);
    assert!(sender.messages().is_empty());
    let record = scenario
        .manager
        .single_validator_batches
        .get(&batch_key(&scenario.metadata, scenario.validator_pubkey))
        .expect("pending record should be retained");
    assert_eq!(*record.state.lock(), SingleValidatorBatchState::Pending);
    drop(record);

    process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(1),
        descriptor.clone(),
    );
    assert_eq!(sender.attempts(), 2);
    let sent = sender.messages();
    assert_eq!(sent.len(), 1);
    let messages = PartialSignatureMessages::from_ssz_bytes(sent[0].ssv_message.data())
        .expect("retried single-validator batch should decode");
    assert_eq!(messages.kind, kind);
    assert_eq!(messages.slot, scenario.metadata.slot);
    assert_eq!(messages.messages.len(), 2);
    assert_eq!(
        messages
            .messages
            .iter()
            .map(|message| message.signing_root)
            .collect::<Vec<_>>(),
        vec![root_0, root_1]
    );
    assert!(messages.messages.iter().all(|message| {
        message.signer == TEST_OPERATOR_ID
            && message.validator_index == ValidatorIndex(42)
            && message.partial_signature == scenario.validator_key.sign(message.signing_root)
    }));
    let record = scenario
        .manager
        .single_validator_batches
        .get(&batch_key(&scenario.metadata, scenario.validator_pubkey))
        .expect("admitted record should be retained");
    assert_eq!(*record.state.lock(), SingleValidatorBatchState::Admitted);
    drop(record);

    let admitted_injections = process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor,
    );
    assert_eq!(admitted_injections.len(), 1);
    assert_eq!(admitted_injections[0].signing_root, root_0);
    assert_eq!(sender.attempts(), 2);
}

#[test]
fn failed_admission_stays_pending_and_sibling_retries() {
    run_failed_admission_and_sibling_retry(PartialSignatureKind::ContributionProofs);
}

#[test]
fn post_consensus_failed_admission_stays_pending_and_sibling_retries() {
    run_failed_admission_and_sibling_retry(PartialSignatureKind::PostConsensus);
}

#[test]
fn post_consensus_construction_failure_retains_pending_for_retry() {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = PartialSignatureKind::PostConsensus;
    let signing_root = Hash256::repeat_byte(0x38);
    let oversized_multiplicity = PartialSignatureMessagesLen::USIZE + 1;

    let injections = process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor(&[(0, signing_root, oversized_multiplicity)]),
    );

    assert_eq!(injections.len(), 1);
    assert_eq!(injections[0].signing_root, signing_root);
    assert_eq!(sender.attempts(), 0);
    let record = scenario
        .manager
        .single_validator_batches
        .get(&batch_key(&scenario.metadata, scenario.validator_pubkey))
        .expect("construction failure should retain the batch record");
    assert_eq!(*record.state.lock(), SingleValidatorBatchState::Pending);
}

fn run_concurrent_callbacks(kind: PartialSignatureKind) {
    let (sender, first_attempt_entered) = BlockingMessageSender::new();
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = kind;
    let descriptor = descriptor(&[
        (0, Hash256::repeat_byte(0x40), 1),
        (1, Hash256::repeat_byte(0x41), 1),
    ]);
    let manager_a = Arc::clone(&scenario.manager);
    let manager_b = Arc::clone(&scenario.manager);
    let metadata_a = scenario.metadata.clone();
    let metadata_b = scenario.metadata.clone();
    let pubkey = scenario.validator_pubkey;
    let key_a = scenario.validator_key.clone();
    let key_b = scenario.validator_key.clone();
    let descriptor_a = descriptor.clone();
    let descriptor_b = descriptor;

    let first = std::thread::spawn(move || {
        process_batch(
            &manager_a,
            &metadata_a,
            pubkey,
            &key_a,
            ValidatorIndex(42),
            SyncSubnetId::new(0),
            descriptor_a,
        )
    });
    first_attempt_entered
        .recv_timeout(Duration::from_secs(5))
        .expect("first callback should enter sign_and_send");
    let record = scenario
        .manager
        .single_validator_batches
        .get(&batch_key(&scenario.metadata, scenario.validator_pubkey))
        .expect("concurrent batch record should exist");
    assert!(
        record.state.try_lock().is_none(),
        "the first callback should hold admission state while sending"
    );
    drop(record);
    let (second_at_lock_tx, second_at_lock_rx) = std_mpsc::sync_channel(1);
    *scenario
        .manager
        .single_validator_batch_before_state_lock
        .lock() = Some(Box::new(move || {
        second_at_lock_tx
            .send(())
            .expect("test should observe the sibling at the state lock");
    }));

    let second = std::thread::spawn(move || {
        process_batch(
            &manager_b,
            &metadata_b,
            pubkey,
            &key_b,
            ValidatorIndex(42),
            SyncSubnetId::new(1),
            descriptor_b,
        )
    });
    second_at_lock_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("sibling callback should reach the state lock during admission");
    assert_eq!(sender.attempts(), 1);
    sender.release();

    assert_eq!(
        first.join().expect("first callback should not panic").len(),
        2
    );
    assert_eq!(
        second
            .join()
            .expect("second callback should not panic")
            .len(),
        1
    );
    assert_eq!(sender.attempts(), 1);
    assert_eq!(
        sender
            .messages
            .lock()
            .expect("message capture lock should not be poisoned")
            .len(),
        1
    );
}

#[test]
fn concurrent_callbacks_produce_one_admitted_send() {
    run_concurrent_callbacks(PartialSignatureKind::ContributionProofs);
}

#[test]
fn concurrent_post_consensus_callbacks_produce_one_admitted_send() {
    run_concurrent_callbacks(PartialSignatureKind::PostConsensus);
}

async fn run_mismatched_callbacks(kind: PartialSignatureKind) {
    let sender = Arc::new(RecordingMessageSender::new(usize::MAX));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = kind;
    let validator_index = ValidatorIndex(42);
    let root_0 = Hash256::repeat_byte(0x50);
    let root_1 = Hash256::repeat_byte(0x51);
    let canonical = descriptor(&[(0, root_0, 1)]);
    scenario.seed_remote_shares(root_0, validator_index);

    sign_and_collect_batch(
        &scenario,
        scenario.metadata.clone(),
        validator_index,
        SyncSubnetId::new(0),
        canonical.clone(),
    )
    .await;
    assert_eq!(sender.attempts(), 1);
    let record_key = batch_key(&scenario.metadata, scenario.validator_pubkey);
    let record = scenario
        .manager
        .single_validator_batches
        .get(&record_key)
        .expect("canonical pending record should be retained");
    assert_eq!(*record.state.lock(), SingleValidatorBatchState::Pending);
    assert_eq!(record.descriptor, canonical);
    drop(record);

    let mismatched = descriptor(&[(1, root_1, 1)]);
    scenario.seed_remote_shares(root_1, validator_index);
    let signature = sign_and_collect_batch(
        &scenario,
        scenario.metadata.clone(),
        validator_index,
        SyncSubnetId::new(1),
        mismatched,
    )
    .await;
    assert_eq!(signature.as_ref(), &scenario.validator_master.sign(root_1));
    assert_eq!(sender.attempts(), 1);

    let mut other_committee = scenario.metadata.clone();
    other_committee.committee_id = CommitteeId([0xAA; 32]);
    let committee_injections = process_batch(
        &scenario.manager,
        &other_committee,
        scenario.validator_pubkey,
        &scenario.validator_key,
        validator_index,
        SyncSubnetId::new(0),
        canonical.clone(),
    );
    let index_injections = process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(43),
        SyncSubnetId::new(0),
        canonical.clone(),
    );
    let other_pubkey = SecretKey::random().public_key().compress();
    let pubkey_mismatch_injections = scenario.manager.process_single_validator_batch(
        &scenario.metadata,
        scenario.validator_pubkey,
        SyncSubnetId::new(0),
        descriptor(&[(0, root_0, 1)]),
        &ValidatorSigningData {
            root: root_0,
            index: validator_index,
            validator_pubkey: other_pubkey,
            share: Some(scenario.validator_key.clone()),
        },
        PartialSignatureMessage {
            partial_signature: scenario.validator_key.sign(root_0),
            signing_root: root_0,
            signer: TEST_OPERATOR_ID,
            validator_index,
        },
    );
    let mut unsupported_kind = scenario.metadata.clone();
    unsupported_kind.kind = PartialSignatureKind::RandaoPartialSig;
    let unsupported_kind_injections = process_batch(
        &scenario.manager,
        &unsupported_kind,
        scenario.validator_pubkey,
        &scenario.validator_key,
        validator_index,
        SyncSubnetId::new(0),
        descriptor(&[(0, root_0, 1)]),
    );
    let mut wrong_role = scenario.metadata.clone();
    wrong_role.role = Role::Aggregator;
    let wrong_role_injections = process_batch(
        &scenario.manager,
        &wrong_role,
        scenario.validator_pubkey,
        &scenario.validator_key,
        validator_index,
        SyncSubnetId::new(0),
        descriptor(&[(0, root_0, 1)]),
    );
    assert_eq!(committee_injections.len(), 1);
    assert_eq!(index_injections.len(), 1);
    assert_eq!(pubkey_mismatch_injections.len(), 1);
    assert_eq!(unsupported_kind_injections.len(), 1);
    assert_eq!(wrong_role_injections.len(), 1);
    assert_eq!(sender.attempts(), 1);
    assert_eq!(scenario.manager.single_validator_batches.len(), 1);
    let record = scenario
        .manager
        .single_validator_batches
        .get(&record_key)
        .expect("mismatches should preserve the canonical pending record");
    assert_eq!(*record.state.lock(), SingleValidatorBatchState::Pending);
    assert_eq!(record.descriptor, canonical);
}

#[tokio::test(flavor = "multi_thread")]
async fn mismatched_callbacks_suppress_publication_but_inject_current_share() {
    run_mismatched_callbacks(PartialSignatureKind::ContributionProofs).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mismatched_post_consensus_callbacks_suppress_publication_but_inject_current_share() {
    run_mismatched_callbacks(PartialSignatureKind::PostConsensus).await;
}

#[test]
fn post_consensus_callback_root_must_be_in_the_descriptor() {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = PartialSignatureKind::PostConsensus;
    let descriptor_root = Hash256::repeat_byte(0x58);
    let callback_root = Hash256::repeat_byte(0x59);

    let injections = process_batch_for_root(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        callback_root,
        descriptor(&[(0, descriptor_root, 1)]),
    );

    assert_eq!(injections.len(), 1);
    assert_eq!(injections[0].signing_root, callback_root);
    assert_eq!(
        injections[0].partial_signature,
        scenario.validator_key.sign(callback_root)
    );
    assert_eq!(sender.attempts(), 0);
    assert!(scenario.manager.single_validator_batches.is_empty());
}

#[test]
fn validator_and_slot_batch_records_are_isolated() {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    let other_key = SecretKey::random();
    let other_pubkey = other_key.public_key().compress();
    let descriptor = descriptor(&[(0, Hash256::repeat_byte(0x60), 1)]);

    process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    process_batch(
        &scenario.manager,
        &scenario.metadata,
        other_pubkey,
        &other_key,
        ValidatorIndex(43),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    let mut next_slot = scenario.metadata.clone();
    next_slot.slot += 1;
    process_batch(
        &scenario.manager,
        &next_slot,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor,
    );

    assert_eq!(sender.attempts(), 3);
    assert_eq!(sender.messages().len(), 3);
    assert_eq!(scenario.manager.single_validator_batches.len(), 3);
}

#[test]
fn contribution_proof_and_post_consensus_records_are_isolated() {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    let type_three_root = Hash256::repeat_byte(0x68);
    let type_zero_root = Hash256::repeat_byte(0x69);
    let mut post_consensus = scenario.metadata.clone();
    post_consensus.kind = PartialSignatureKind::PostConsensus;

    process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor(&[(0, type_three_root, 1)]),
    );
    process_batch(
        &scenario.manager,
        &post_consensus,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor(&[(0, type_zero_root, 1)]),
    );

    assert_eq!(scenario.manager.single_validator_batches.len(), 2);
    assert!(
        scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key(&scenario.metadata, scenario.validator_pubkey))
    );
    assert!(
        scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key(&post_consensus, scenario.validator_pubkey))
    );
    let kinds = sender
        .messages()
        .iter()
        .map(|unsigned| {
            PartialSignatureMessages::from_ssz_bytes(unsigned.ssv_message.data())
                .expect("phase-isolated batch should decode")
                .kind
        })
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            PartialSignatureKind::ContributionProofs,
            PartialSignatureKind::PostConsensus
        ]
    );
}

#[test]
fn cleanup_removes_pending_and_admitted_records_from_both_phases() {
    let sender = Arc::new(RecordingMessageSender::new(1));
    let scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    let descriptor = descriptor(&[(0, Hash256::repeat_byte(0x70), 1)]);
    let mut stale = scenario.metadata.clone();
    stale.slot = BATCH_TEST_SLOT - 2;
    let mut stale_post_consensus = stale.clone();
    stale_post_consensus.kind = PartialSignatureKind::PostConsensus;
    scenario.manager.slot_clock.set_slot(stale.slot.as_u64());

    process_batch(
        &scenario.manager,
        &stale,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    process_batch(
        &scenario.manager,
        &stale_post_consensus,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    scenario
        .manager
        .slot_clock
        .set_slot(BATCH_TEST_SLOT.as_u64());
    process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor,
    );
    assert_eq!(scenario.manager.single_validator_batches.len(), 3);

    scenario
        .manager
        .remove_stale_entries(BATCH_TEST_SLOT.saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS));

    assert_eq!(scenario.manager.single_validator_batches.len(), 1);
    assert!(
        scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key(&scenario.metadata, scenario.validator_pubkey))
    );
}

#[test]
fn cleanup_removes_pending_and_admitted_batch_records() {
    let sender = Arc::new(RecordingMessageSender::new(1));
    let scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    let second_key = SecretKey::random();
    let second_pubkey = second_key.public_key().compress();
    let descriptor = descriptor(&[(0, Hash256::repeat_byte(0x70), 1)]);
    let mut stale = scenario.metadata.clone();
    stale.slot = BATCH_TEST_SLOT - 2;
    scenario.manager.slot_clock.set_slot(stale.slot.as_u64());

    process_batch(
        &scenario.manager,
        &stale,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    process_batch(
        &scenario.manager,
        &stale,
        second_pubkey,
        &second_key,
        ValidatorIndex(43),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    scenario
        .manager
        .slot_clock
        .set_slot(BATCH_TEST_SLOT.as_u64());
    process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor,
    );
    assert_eq!(scenario.manager.single_validator_batches.len(), 3);

    scenario
        .manager
        .remove_stale_entries(BATCH_TEST_SLOT.saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS));

    assert_eq!(scenario.manager.single_validator_batches.len(), 1);
    assert!(
        scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key(&scenario.metadata, scenario.validator_pubkey))
    );
}

fn run_stale_callback_after_cleanup(kind: PartialSignatureKind) {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = kind;
    let signing_root = Hash256::repeat_byte(0x71);
    let descriptor = descriptor(&[(0, signing_root, 1)]);
    let batch_key = batch_key(&scenario.metadata, scenario.validator_pubkey);

    let initial_injections = process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    assert_eq!(initial_injections.len(), 1);
    assert_eq!(sender.attempts(), 1);
    assert!(
        scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key)
    );

    let retained_slot = BATCH_TEST_SLOT + SIGNATURE_COLLECTOR_RETAIN_SLOTS;
    scenario.manager.slot_clock.set_slot(retained_slot.as_u64());
    let retained_cutoff = retained_slot.saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS);
    assert_eq!(retained_cutoff, BATCH_TEST_SLOT);
    scenario.manager.remove_stale_entries(retained_cutoff);
    assert!(
        scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key)
    );
    let retained_injections = process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor.clone(),
    );
    assert_eq!(retained_injections.len(), 1);
    assert_eq!(retained_injections[0].signing_root, signing_root);
    assert_eq!(sender.attempts(), 1);
    assert!(
        scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key)
    );

    let current_slot = retained_slot + 1;
    scenario.manager.slot_clock.set_slot(current_slot.as_u64());
    let cutoff = current_slot.saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS);
    assert_eq!(cutoff, BATCH_TEST_SLOT + 1);
    scenario.manager.remove_stale_entries(cutoff);
    assert!(
        !scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key)
    );

    let injections = process_batch(
        &scenario.manager,
        &scenario.metadata,
        scenario.validator_pubkey,
        &scenario.validator_key,
        ValidatorIndex(42),
        SyncSubnetId::new(0),
        descriptor,
    );

    assert!(injections.is_empty());
    assert_eq!(sender.attempts(), 1);
    assert!(
        !scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key)
    );
}

#[test]
fn stale_callback_after_cleanup_does_not_recreate_or_publish_batch() {
    run_stale_callback_after_cleanup(PartialSignatureKind::ContributionProofs);
}

#[test]
fn stale_post_consensus_callback_after_cleanup_does_not_recreate_or_publish_batch() {
    run_stale_callback_after_cleanup(PartialSignatureKind::PostConsensus);
}

async fn run_stale_sign_and_collect(kind: PartialSignatureKind) {
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario =
        BatchScenario::new_with_max_workers(Arc::clone(&sender) as Arc<dyn MessageSender>, 1);
    scenario.metadata.kind = kind;
    let validator_index = ValidatorIndex(42);
    let signing_root = Hash256::repeat_byte(0x72);
    let descriptor = descriptor(&[(0, signing_root, 1)]);
    let collector_key = (signing_root, validator_index);
    let batch_key = batch_key(&scenario.metadata, scenario.validator_pubkey);

    drop(
        scenario
            .manager
            .get_or_spawn(signing_root, validator_index, scenario.metadata.slot),
    );
    assert!(
        scenario
            .manager
            .signature_collectors
            .contains_key(&collector_key)
    );

    let current_slot = BATCH_TEST_SLOT + SIGNATURE_COLLECTOR_RETAIN_SLOTS + 1;
    scenario.manager.slot_clock.set_slot(current_slot.as_u64());
    let cutoff = current_slot.saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS);
    assert_eq!(cutoff, BATCH_TEST_SLOT + 1);
    scenario.manager.remove_stale_entries(cutoff);
    assert!(
        !scenario
            .manager
            .signature_collectors
            .contains_key(&collector_key)
    );

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        scenario.manager.sign_and_collect(
            scenario.metadata.clone(),
            SignatureRequester::SingleValidatorBatch {
                pubkey: scenario.validator_pubkey,
                subnet_id: SyncSubnetId::new(0),
                descriptor,
            },
            ValidatorSigningData {
                root: signing_root,
                index: validator_index,
                validator_pubkey: scenario.validator_pubkey,
                share: Some(scenario.validator_key.clone()),
            },
        ),
    )
    .await
    .expect("stale notifier should be dropped promptly");
    assert!(matches!(result, Err(CollectionError::QueueClosedError)));

    drain_batch_processor_work(&scenario.manager).await;

    assert_eq!(sender.attempts(), 0);
    assert!(
        !scenario
            .manager
            .signature_collectors
            .contains_key(&collector_key)
    );
    assert!(
        !scenario
            .manager
            .single_validator_batches
            .contains_key(&batch_key)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_sign_and_collect_does_not_recreate_cleaned_collectors() {
    run_stale_sign_and_collect(PartialSignatureKind::ContributionProofs).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_post_consensus_sign_and_collect_does_not_recreate_cleaned_collectors() {
    run_stale_sign_and_collect(PartialSignatureKind::PostConsensus).await;
}

/// Signs `signing_root` for the scenario validator as a standalone single-validator request and
/// waits for the reconstructed signature.
async fn sign_and_collect_single_validator(
    scenario: &BatchScenario,
    signing_root: Hash256,
    validator_index: ValidatorIndex,
) -> Result<Arc<Signature>, CollectionError> {
    tokio::time::timeout(
        Duration::from_secs(5),
        scenario.manager.sign_and_collect(
            scenario.metadata.clone(),
            SignatureRequester::SingleValidator {
                pubkey: scenario.validator_pubkey,
            },
            ValidatorSigningData {
                root: signing_root,
                index: validator_index,
                validator_pubkey: scenario.validator_pubkey,
                share: Some(scenario.validator_key.clone()),
            },
        ),
    )
    .await
    .expect("single-validator request should resolve promptly")
}

/// SIP-94 §7: an early network partial creates a collector before any local request,
/// and cleanup retains that collector through the slot following its proposal.
#[tokio::test(flavor = "multi_thread")]
async fn early_network_partial_retained_through_slot_after_proposal() {
    // Arrange: receipt is at epoch 2 start and the proposal is epoch 3's last slot.
    let sender = Arc::new(RecordingMessageSender::new(0));
    let scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    let receipt_slot = BATCH_TEST_SLOT;
    let proposal_slot = receipt_slot + 2 * SLOTS_PER_EPOCH - 1;
    let signing_root = Hash256::repeat_byte(0xE1);
    let validator_index = ValidatorIndex(7);
    let collector_key = (signing_root, validator_index);
    let (operator_id, share) = &scenario.remote_shares[0];
    assert!(scenario.manager.signature_collectors.is_empty());

    // Act: ingest only a network partial, without calling sign_and_collect locally.
    scenario
        .manager
        .receive_partial_signatures(PartialSignatureMessages {
            kind: PartialSignatureKind::ProposerPreferences,
            slot: proposal_slot,
            messages: VariableList::new(vec![PartialSignatureMessage {
                partial_signature: share.sign(signing_root),
                signing_root,
                signer: *operator_id,
                validator_index,
            }])
            .expect("one partial fits in a packet"),
        })
        .expect("network partial should enter the processor");
    drain_batch_processor_work(&scenario.manager).await;

    // Assert: the network path creates a future-slot collector and publishes nothing.
    assert_eq!(scenario.manager.signature_collectors.len(), 1);
    assert_eq!(
        scenario
            .manager
            .signature_collectors
            .get(&collector_key)
            .expect("network partial must create an unrequested collector")
            .for_slot,
        proposal_slot,
    );
    assert_eq!(sender.attempts(), 0);

    // Act and assert: run production cleanup at the relevant slot boundaries.
    for (current_slot, retained) in [
        (receipt_slot, true),
        (proposal_slot - 1, true),
        (proposal_slot, true),
        (proposal_slot + 1, true),
        (proposal_slot + 2, false),
    ] {
        scenario.manager.slot_clock.set_slot(current_slot.as_u64());
        let cutoff = scenario
            .manager
            .slot_clock
            .now()
            .expect("clock is available")
            .saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS);
        scenario.manager.remove_stale_entries(cutoff);
        assert_eq!(
            scenario
                .manager
                .signature_collectors
                .contains_key(&collector_key),
            retained,
            "unexpected collector retention after cleanup at slot {current_slot}"
        );
    }
}

/// SIP-94 section 5: a peer authenticates against builders A, B, and C and gossips all three
/// RequestAuth shares in one packet. Our node is configured for A and B only, so C never gets a
/// `sign_and_collect` registration. The unrequested C entries must not stop A and B from
/// reconstructing.
#[tokio::test(flavor = "multi_thread")]
async fn request_auth_batch_unrequested_root_does_not_block_requested_roots() {
    // Arrange: one multi-entry RequestAuth packet per remote operator covering A, B, and C.
    let sender = Arc::new(RecordingMessageSender::new(0));
    let mut scenario = BatchScenario::new(Arc::clone(&sender) as Arc<dyn MessageSender>);
    scenario.metadata.kind = PartialSignatureKind::RequestAuth;
    scenario.metadata.role = Role::ProposerPreferences;
    let validator_index = ValidatorIndex(7);
    let root_a = Hash256::repeat_byte(0xA1);
    let root_b = Hash256::repeat_byte(0xB2);
    let root_c = Hash256::repeat_byte(0xC3);
    let packet_roots = [root_a, root_b, root_c];

    for (operator_id, share) in &scenario.remote_shares {
        let entries = packet_roots
            .iter()
            .map(|signing_root| PartialSignatureMessage {
                partial_signature: share.sign(*signing_root),
                signing_root: *signing_root,
                signer: *operator_id,
                validator_index,
            })
            .collect::<Vec<_>>();
        scenario
            .manager
            .receive_partial_signatures(PartialSignatureMessages {
                kind: PartialSignatureKind::RequestAuth,
                slot: scenario.metadata.slot,
                messages: VariableList::new(entries)
                    .expect("three entries fit in a partial signature packet"),
            })
            .expect("remote RequestAuth packet should enter the processor");
    }

    // Act: register and sign only the roots this node requested.
    let (signature_a, signature_b) = tokio::join!(
        sign_and_collect_single_validator(&scenario, root_a, validator_index),
        sign_and_collect_single_validator(&scenario, root_b, validator_index),
    );

    // Assert: both requested roots reconstruct to the master signature.
    let signature_a = signature_a.expect("root A should reconstruct");
    let signature_b = signature_b.expect("root B should reconstruct");
    assert_eq!(
        signature_a.as_ref(),
        &scenario.validator_master.sign(root_a)
    );
    assert_eq!(
        signature_b.as_ref(),
        &scenario.validator_master.sign(root_b)
    );

    // Assert: C only ever received remote shares. Registration state lives inside the collector
    // task, so the observable evidence is the share-only collector entry plus our two outgoing
    // partials (A and B, never C).
    assert!(
        scenario
            .manager
            .signature_collectors
            .contains_key(&(root_c, validator_index)),
        "unrequested root C should keep a share-only collector"
    );
    let sent = sender.messages();
    assert_eq!(
        sent.len(),
        2,
        "only the requested roots A and B should be published"
    );
    let mut sent_roots = sent
        .iter()
        .map(|unsigned| {
            let messages = PartialSignatureMessages::from_ssz_bytes(unsigned.ssv_message.data())
                .expect("single-validator RequestAuth message should decode");
            assert_eq!(messages.kind, PartialSignatureKind::RequestAuth);
            assert_eq!(messages.messages.len(), 1);
            messages.messages[0].signing_root
        })
        .collect::<Vec<_>>();
    sent_roots.sort();
    let mut expected_roots = vec![root_a, root_b];
    expected_roots.sort();
    assert_eq!(sent_roots, expected_roots);
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
    state.add_partial_signature(keys.shares[2].0, keys.shares[2].1.sign(WRONG_ROOT), None);
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
    state.add_partial_signature(invalid_operator, Signature::empty(), None);
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
    state.add_partial_signature(keys.shares[3].0, keys.shares[3].1.sign(WRONG_ROOT), None);
    state.add_partial_signature(keys.shares[4].0, Signature::empty(), None);

    let (notify, mut receiver) = oneshot::channel();
    assert!(
        state
            .register_request(Some(notify), THRESHOLD, validator_pubkey, None)
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
    state.add_partial_signature(keys.shares[2].0, keys.shares[2].1.sign(SIGNING_ROOT), None);
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
            .register_request(Some(notify), THRESHOLD, malformed, None)
            .is_break()
    );

    let mut threshold_state = SignatureCollectorState::new(SIGNING_ROOT);
    let _receiver = register_notifier(&mut threshold_state, validator_pubkey);
    let (notify, _) = oneshot::channel();
    assert!(
        threshold_state
            .register_request(Some(notify), THRESHOLD + 1, validator_pubkey, None)
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
            .register_request(Some(notify), THRESHOLD, conflicting_pubkey, None)
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
    empty_state.add_partial_signature(keys.shares[2].0, keys.shares[2].1.sign(WRONG_ROOT), None);
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
        notify: Some(notify),
        required_companion: None,
        threshold: THRESHOLD,
        validator_pubkey,
    });
    for (operator_id, secret_key) in &keys.shares[..(THRESHOLD - 1) as usize] {
        send(CollectorMessageKind::PartialSignature {
            companion_root: None,
            operator_id: *operator_id,
            signature: Box::new(secret_key.sign(SIGNING_ROOT)),
        });
    }
    send(CollectorMessageKind::PartialSignature {
        companion_root: None,
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
        notify: Some(notify),
        required_companion: None,
        threshold: THRESHOLD,
        validator_pubkey,
    });
    for (operator_id, secret_key) in &keys.shares[..THRESHOLD as usize] {
        send(CollectorMessageKind::PartialSignature {
            companion_root: None,
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
        notify: Some(notify),
        required_companion: None,
        threshold: THRESHOLD,
        validator_pubkey,
    });
    for (operator_id, secret_key) in &keys.shares[..(THRESHOLD - 1) as usize] {
        send(CollectorMessageKind::PartialSignature {
            companion_root: None,
            operator_id: *operator_id,
            signature: Box::new(secret_key.sign(SIGNING_ROOT)),
        });
    }
    send(CollectorMessageKind::PartialSignature {
        companion_root: None,
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

// ==================== Proposer block and envelope packets ====================

const PROPOSER_BLOCK_ROOT: Hash256 = Hash256::repeat_byte(0xB1);
const PROPOSER_ENVELOPE_ROOT: Hash256 = Hash256::repeat_byte(0xE1);
const PROPOSER_VALIDATOR_INDEX: ValidatorIndex = ValidatorIndex(42);
const SECOND_COMPANION_ROOT: Hash256 = Hash256::repeat_byte(0xB2);
const THIRD_COMPANION_ROOT: Hash256 = Hash256::repeat_byte(0xB3);

fn proposer_scenario(sender: Arc<dyn MessageSender>) -> BatchScenario {
    let mut scenario = BatchScenario::new(sender);
    scenario.metadata.kind = PartialSignatureKind::PostConsensus;
    scenario.metadata.role = Role::Proposer;
    scenario
}

fn proposer_signing_data(scenario: &BatchScenario, root: Hash256) -> ValidatorSigningData {
    ValidatorSigningData {
        root,
        index: PROPOSER_VALIDATOR_INDEX,
        validator_pubkey: scenario.validator_pubkey,
        share: Some(scenario.validator_key.clone()),
    }
}

fn seed_proposer_packets(scenario: &BatchScenario, roots: &[Hash256]) {
    for (operator_id, share) in &scenario.remote_shares {
        scenario
            .manager
            .receive_proposer_partial_signatures(PartialSignatureMessages {
                kind: PartialSignatureKind::PostConsensus,
                slot: scenario.metadata.slot,
                messages: VariableList::new(
                    roots
                        .iter()
                        .map(|root| PartialSignatureMessage {
                            partial_signature: share.sign(*root),
                            signing_root: *root,
                            signer: *operator_id,
                            validator_index: PROPOSER_VALIDATOR_INDEX,
                        })
                        .collect(),
                )
                .expect("proposer packet must fit"),
            })
            .expect("proposer packet should enter the processor");
    }
}

async fn sign_proposer_packet(scenario: &BatchScenario, with_envelope: bool) -> Arc<Signature> {
    tokio::time::timeout(
        Duration::from_secs(5),
        scenario.manager.sign_proposer_packet(
            scenario.metadata.clone(),
            scenario.validator_pubkey,
            proposer_signing_data(scenario, PROPOSER_BLOCK_ROOT),
            with_envelope.then(|| proposer_signing_data(scenario, PROPOSER_ENVELOPE_ROOT)),
        ),
    )
    .await
    .expect("block quorum should finish independently of the envelope")
    .expect("proposer block signature should reconstruct")
}

async fn wait_for_proposer_envelope(scenario: &BatchScenario) -> Arc<Signature> {
    tokio::time::timeout(
        Duration::from_secs(5),
        scenario.manager.wait_for_registered_signature(
            scenario.metadata.clone(),
            PROPOSER_VALIDATOR_INDEX,
            scenario.validator_pubkey,
            PROPOSER_ENVELOPE_ROOT,
            Some(PROPOSER_BLOCK_ROOT),
        ),
    )
    .await
    .expect("envelope quorum should finish")
    .expect("registered envelope signature should remain available")
}

/// Early authenticated packets can list the block or the envelope first. Both roots must
/// reconstruct with the local share, while local signing emits exactly one two-entry packet.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_pair_sends_once_and_retains_early_shares_in_both_orders() {
    for roots in [
        [PROPOSER_BLOCK_ROOT, PROPOSER_ENVELOPE_ROOT],
        [PROPOSER_ENVELOPE_ROOT, PROPOSER_BLOCK_ROOT],
    ] {
        // Arrange: remote quorum needs our locally injected shares to complete.
        let sender = Arc::new(RecordingMessageSender::new(0));
        let scenario = proposer_scenario(Arc::clone(&sender) as Arc<dyn MessageSender>);
        seed_proposer_packets(&scenario, &roots);
        drain_batch_processor_work(&scenario.manager).await;

        // Act: one signing operation registers both roots; a later callback only waits.
        let block = sign_proposer_packet(&scenario, true).await;
        let envelope = wait_for_proposer_envelope(&scenario).await;
        let repeated_envelope = wait_for_proposer_envelope(&scenario).await;

        // Assert: independent results and one local wire packet, including both local shares.
        assert_eq!(
            block.as_ref(),
            &scenario.validator_master.sign(PROPOSER_BLOCK_ROOT)
        );
        assert_eq!(
            envelope.as_ref(),
            &scenario.validator_master.sign(PROPOSER_ENVELOPE_ROOT)
        );
        assert_eq!(repeated_envelope, envelope);
        assert_eq!(
            sender.attempts(),
            1,
            "wait-only callbacks must not broadcast"
        );
        let messages = sender.messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].ssv_message.msg_id().role(),
            Some(Role::Proposer)
        );
        let packet = PartialSignatureMessages::from_ssz_bytes(messages[0].ssv_message.data())
            .expect("local proposer payload should decode");
        assert_eq!(packet.kind, PartialSignatureKind::PostConsensus);
        assert_eq!(packet.messages.len(), 2);
        assert_eq!(
            packet
                .messages
                .iter()
                .map(|entry| entry.signing_root)
                .collect::<Vec<_>>(),
            vec![PROPOSER_BLOCK_ROOT, PROPOSER_ENVELOPE_ROOT],
        );
        for entry in &packet.messages {
            assert_eq!(entry.signer, TEST_OPERATOR_ID);
            assert_eq!(entry.validator_index, PROPOSER_VALIDATOR_INDEX);
            assert_eq!(
                entry.partial_signature,
                scenario.validator_key.sign(entry.signing_root)
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn proposer_block_only_packet_finishes_without_creating_envelope_collector() {
    // Arrange: external-builder proposals carry only block shares.
    let sender = Arc::new(RecordingMessageSender::new(0));
    let scenario = proposer_scenario(Arc::clone(&sender) as Arc<dyn MessageSender>);
    seed_proposer_packets(&scenario, &[PROPOSER_BLOCK_ROOT]);

    // Act: no envelope signing data is supplied.
    let block = sign_proposer_packet(&scenario, false).await;

    // Assert: one root, one packet, and no unused envelope registration.
    assert_eq!(
        block.as_ref(),
        &scenario.validator_master.sign(PROPOSER_BLOCK_ROOT)
    );
    assert!(
        !scenario
            .manager
            .signature_collectors
            .contains_key(&(PROPOSER_ENVELOPE_ROOT, PROPOSER_VALIDATOR_INDEX,))
    );
    let messages = sender.messages();
    assert_eq!(messages.len(), 1);
    let packet = PartialSignatureMessages::from_ssz_bytes(messages[0].ssv_message.data())
        .expect("block-only packet should decode");
    assert_eq!(packet.messages.len(), 1);
    assert_eq!(packet.messages[0].signing_root, PROPOSER_BLOCK_ROOT);
}

/// Block-only final packets can complete the block before any remote envelope share arrives.
#[tokio::test(flavor = "multi_thread")]
async fn proposer_block_threshold_does_not_wait_for_envelope_threshold() {
    // Arrange: enough block shares, only our local envelope share.
    let sender = Arc::new(RecordingMessageSender::new(0));
    let scenario = proposer_scenario(Arc::clone(&sender) as Arc<dyn MessageSender>);
    seed_proposer_packets(&scenario, &[PROPOSER_BLOCK_ROOT]);

    // Act: block completion must precede the later envelope quorum.
    let block = sign_proposer_packet(&scenario, true).await;
    assert_eq!(
        block.as_ref(),
        &scenario.validator_master.sign(PROPOSER_BLOCK_ROOT)
    );
    seed_proposer_packets(&scenario, &[PROPOSER_ENVELOPE_ROOT, PROPOSER_BLOCK_ROOT]);
    let envelope = wait_for_proposer_envelope(&scenario).await;

    // Assert: waiting for the second root never produces another packet.
    assert_eq!(
        envelope.as_ref(),
        &scenario.validator_master.sign(PROPOSER_ENVELOPE_ROOT)
    );
    assert_eq!(sender.attempts(), 1);
}

#[test]
fn proposer_envelope_rejects_unpaired_and_wrong_companion_shares_before_and_after_registration() {
    for early in [true, false] {
        for companion in [None, Some(WRONG_ROOT)] {
            // Arrange: all signatures are cryptographically valid, but their packet is invalid.
            let keys = split_random_master();
            let mut state = SignatureCollectorState::new(SIGNING_ROOT);
            if !early {
                assert!(
                    state
                        .register_request(
                            None,
                            THRESHOLD,
                            keys.master.public_key().compress(),
                            Some(PROPOSER_BLOCK_ROOT)
                        )
                        .is_continue()
                );
            }
            for (operator, share) in &keys.shares[..THRESHOLD as usize] {
                state.add_partial_signature(*operator, share.sign(SIGNING_ROOT), companion);
            }

            // Act: register the decided block requirement after any early arrivals.
            if early {
                assert!(
                    state
                        .register_request(
                            None,
                            THRESHOLD,
                            keys.master.public_key().compress(),
                            Some(PROPOSER_BLOCK_ROOT)
                        )
                        .is_continue()
                );
            }

            // Assert: BLS validity alone cannot satisfy the envelope quorum.
            assert!(matches!(state.try_reconstruct(), Ok(None)));
            assert!(state.full_signature.is_none());
        }
    }
}

#[test]
fn proposer_identical_share_from_second_round_can_supply_missing_companion() {
    for registered_before_second_round in [true, false] {
        // Arrange: the first packet carries the wrong block root.
        let keys = split_random_master();
        let mut state = SignatureCollectorState::new(SIGNING_ROOT);
        for (operator, share) in &keys.shares[..THRESHOLD as usize] {
            state.add_partial_signature(*operator, share.sign(SIGNING_ROOT), Some(WRONG_ROOT));
        }
        if registered_before_second_round {
            assert!(
                state
                    .register_request(
                        None,
                        THRESHOLD,
                        keys.master.public_key().compress(),
                        Some(PROPOSER_BLOCK_ROOT)
                    )
                    .is_continue()
            );
            assert!(matches!(state.try_reconstruct(), Ok(None)));
        }

        // Act: a permitted later-round packet has identical envelope bytes and the decided block.
        for (operator, share) in &keys.shares[..THRESHOLD as usize] {
            state.add_partial_signature(
                *operator,
                share.sign(SIGNING_ROOT),
                Some(PROPOSER_BLOCK_ROOT),
            );
        }
        if !registered_before_second_round {
            assert!(
                state
                    .register_request(
                        None,
                        THRESHOLD,
                        keys.master.public_key().compress(),
                        Some(PROPOSER_BLOCK_ROOT)
                    )
                    .is_continue()
            );
        }
        complete_ready_reconstruction(&mut state);

        // Assert: the second packet supplies the required provenance without duplicate weight.
        assert_eq!(
            state.full_signature.as_deref(),
            Some(&keys.master.sign(SIGNING_ROOT))
        );
    }
}

#[test]
fn proposer_companion_provenance_is_bound_to_signature_bytes() {
    // Arrange: a valid envelope share first arrives with the wrong companion.
    let keys = split_random_master();
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);
    for (operator, share) in &keys.shares[..THRESHOLD as usize] {
        state.add_partial_signature(*operator, share.sign(SIGNING_ROOT), Some(WRONG_ROOT));
    }

    // Act: different signature bytes from those operators claim the expected companion.
    for (operator, share) in &keys.shares[..THRESHOLD as usize] {
        state.add_partial_signature(*operator, share.sign(WRONG_ROOT), Some(PROPOSER_BLOCK_ROOT));
    }
    assert!(
        state
            .register_request(
                None,
                THRESHOLD,
                keys.master.public_key().compress(),
                Some(PROPOSER_BLOCK_ROOT)
            )
            .is_continue()
    );

    // Assert: no root evidence from the conflicting signature transfers to the first signature.
    assert!(matches!(state.try_reconstruct(), Ok(None)));
}

#[test]
fn proposer_companion_provenance_keeps_two_distinct_roots_and_ignores_duplicates() {
    for expected_companion in [SECOND_COMPANION_ROOT, THIRD_COMPANION_ROOT] {
        // Arrange: before decision, each share sees one repeated and two distinct companions.
        let keys = split_random_master();
        let mut state = SignatureCollectorState::new(SIGNING_ROOT);
        for companion in [
            PROPOSER_BLOCK_ROOT,
            PROPOSER_BLOCK_ROOT,
            SECOND_COMPANION_ROOT,
            THIRD_COMPANION_ROOT,
        ] {
            for (operator, share) in &keys.shares[..THRESHOLD as usize] {
                state.add_partial_signature(*operator, share.sign(SIGNING_ROOT), Some(companion));
            }
        }

        // Act: bind this root to a decided block after the early packets.
        assert!(
            state
                .register_request(
                    None,
                    THRESHOLD,
                    keys.master.public_key().compress(),
                    Some(expected_companion)
                )
                .is_continue()
        );

        // Assert: duplicate evidence used no slot; a third distinct witness exceeded the bound.
        if expected_companion == SECOND_COMPANION_ROOT {
            complete_ready_reconstruction(&mut state);
            assert_eq!(
                state.full_signature.as_deref(),
                Some(&keys.master.sign(SIGNING_ROOT))
            );
        } else {
            assert!(matches!(state.try_reconstruct(), Ok(None)));
        }
    }
}

#[test]
fn proposer_without_notifier_caches_result_for_a_later_waiter() {
    // Arrange: the envelope is registered eagerly, without a waiting callback.
    let keys = split_random_master();
    let validator_pubkey = keys.master.public_key().compress();
    let mut state = SignatureCollectorState::new(SIGNING_ROOT);
    assert!(
        state
            .register_request(None, THRESHOLD, validator_pubkey, Some(PROPOSER_BLOCK_ROOT))
            .is_continue()
    );
    for (operator, share) in &keys.shares[..THRESHOLD as usize] {
        state.add_partial_signature(
            *operator,
            share.sign(SIGNING_ROOT),
            Some(PROPOSER_BLOCK_ROOT),
        );
    }

    // Act: reconstruct before Lighthouse asks for the envelope, then register the callback.
    complete_ready_reconstruction(&mut state);
    let (notify, mut receiver) = oneshot::channel();
    assert!(
        state
            .register_request(
                Some(notify),
                THRESHOLD,
                validator_pubkey,
                Some(PROPOSER_BLOCK_ROOT)
            )
            .is_continue()
    );

    // Assert: the retained signature resolves the callback immediately.
    expect_signature(&mut receiver, &keys.master.sign(SIGNING_ROOT));
}

#[test]
fn proposer_admission_policy_cannot_change_before_or_after_reconstruction() {
    for cached in [false, true] {
        for (initial, conflicting) in [
            (None, Some(PROPOSER_BLOCK_ROOT)),
            (Some(PROPOSER_BLOCK_ROOT), None),
            (Some(PROPOSER_BLOCK_ROOT), Some(WRONG_ROOT)),
        ] {
            // Arrange: first registration fixes the policy for this root.
            let keys = split_random_master();
            let pubkey = keys.master.public_key().compress();
            let mut state = SignatureCollectorState::new(SIGNING_ROOT);
            assert!(
                state
                    .register_request(None, THRESHOLD, pubkey, initial)
                    .is_continue()
            );
            if cached {
                for (operator, share) in &keys.shares[..THRESHOLD as usize] {
                    state.add_partial_signature(*operator, share.sign(SIGNING_ROOT), initial);
                }
                complete_ready_reconstruction(&mut state);
            }

            // Act: a later caller tries to weaken, strengthen, or change the companion policy.
            let (notify, mut receiver) = oneshot::channel();
            let result = state.register_request(Some(notify), THRESHOLD, pubkey, conflicting);

            // Assert: even a cached signature must not bypass registration compatibility.
            assert!(result.is_break());
            assert!(matches!(
                receiver.try_recv(),
                Err(oneshot::error::TryRecvError::Closed)
            ));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn proposer_wait_after_expiry_does_not_recreate_collector_or_send() {
    // Arrange: both roots completed, then normal slot cleanup expires them.
    let sender = Arc::new(RecordingMessageSender::new(0));
    let scenario = proposer_scenario(Arc::clone(&sender) as Arc<dyn MessageSender>);
    seed_proposer_packets(&scenario, &[PROPOSER_BLOCK_ROOT, PROPOSER_ENVELOPE_ROOT]);
    sign_proposer_packet(&scenario, true).await;
    wait_for_proposer_envelope(&scenario).await;
    let current_slot = scenario.metadata.slot + SIGNATURE_COLLECTOR_RETAIN_SLOTS + 1;
    scenario.manager.slot_clock.set_slot(current_slot.as_u64());
    scenario
        .manager
        .remove_stale_entries(current_slot.saturating_sub(SIGNATURE_COLLECTOR_RETAIN_SLOTS));
    assert!(scenario.manager.signature_collectors.is_empty());

    // Act: the delayed callback attempts a lookup after the retention window.
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        scenario.manager.wait_for_registered_signature(
            scenario.metadata.clone(),
            PROPOSER_VALIDATOR_INDEX,
            scenario.validator_pubkey,
            PROPOSER_ENVELOPE_ROOT,
            Some(PROPOSER_BLOCK_ROOT),
        ),
    )
    .await
    .expect("an expired lookup should fail promptly");
    drain_batch_processor_work(&scenario.manager).await;

    // Assert: wait-only lookup cannot resurrect state or produce a fresh local packet.
    assert!(result.is_err());
    assert!(scenario.manager.signature_collectors.is_empty());
    assert_eq!(sender.attempts(), 1);
}
