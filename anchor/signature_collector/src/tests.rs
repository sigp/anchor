use std::time::Duration;

use bls::{PublicKeyBytes, SecretKey, Signature};
use bls_lagrange::{KeyId, split_with_rng};
use database::OwnOperatorId;
use fork::{Fork, ForkSchedule};
use message_sender::{Error as MessageSenderError, MessageSender};
use parking_lot::Mutex;
use processor::{Config as ProcessorConfig, spawn as spawn_processor};
use rand::{prelude::*, rngs::StdRng};
use slot_clock::ManualSlotClock;
use ssv_types::{domain_type::DomainType, message::SignedSSVMessage};
use ssz::Decode;
use task_executor::TaskExecutor;
use tokio::sync::oneshot;

use super::*;

const TEST_RNG_SEED: u64 = 0xDEAD_BEEF_CAFE_0001;
const TOTAL_SHARES: u64 = 4;
const THRESHOLD: u64 = 3;
const SIGNING_ROOT: Hash256 = Hash256::repeat_byte(0xAB);
const TEST_OPERATOR_ID: OperatorId = OperatorId(1);
const TEST_SLOT: Slot = Slot::new(64);
const SLOTS_PER_EPOCH: u64 = 32;

#[derive(Default)]
struct CapturingMessageSender {
    messages: Mutex<Vec<UnsignedSSVMessage>>,
}

impl CapturingMessageSender {
    fn messages(&self) -> Vec<UnsignedSSVMessage> {
        self.messages.lock().clone()
    }
}

impl MessageSender for CapturingMessageSender {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        _committee_id: CommitteeId,
        _additional_message_callback: Option<Box<dyn FnOnce(&SignedSSVMessage) + Send + 'static>>,
    ) -> Result<(), MessageSenderError> {
        self.messages.lock().push(message);
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

fn create_test_executor() -> (TaskExecutor, async_channel::Sender<()>) {
    let handle = tokio::runtime::Handle::current();
    let (signal, exit) = async_channel::bounded::<()>(1);
    let (shutdown, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown);
    (executor, signal)
}

fn new_test_manager(
    message_sender: Arc<CapturingMessageSender>,
) -> (
    Arc<SignatureCollectorManager<ManualSlotClock>>,
    async_channel::Sender<()>,
) {
    let (executor, exit_signal) = create_test_executor();
    let processor = spawn_processor(ProcessorConfig::default(), executor);
    let fork_schedule = Arc::new(ForkSchedule::new(Fork::Alan, DomainType::default(), "test"));
    let slot_clock = ManualSlotClock::new(
        Slot::new(0),
        Duration::from_secs(0),
        Duration::from_secs(12),
    );

    let manager = SignatureCollectorManager::new(
        processor,
        OwnOperatorId::Known(TEST_OPERATOR_ID),
        fork_schedule,
        SLOTS_PER_EPOCH,
        message_sender,
        slot_clock,
    )
    .expect("manager should be created");

    (manager, exit_signal)
}

fn split_random_master() -> Vec<(OperatorId, SecretKey)> {
    let rng = &mut StdRng::seed_from_u64(TEST_RNG_SEED);
    let master = SecretKey::random();

    split_with_rng(
        &master,
        THRESHOLD,
        (1..=TOTAL_SHARES).map(|x| KeyId::try_from(x).unwrap()),
        rng,
    )
    .expect("split should succeed")
    .into_iter()
    .map(|(kid, sk)| (OperatorId(u64::from(kid)), sk))
    .collect()
}

fn register_notifier(
    state: &mut SignatureCollectorState,
    threshold: u64,
) -> oneshot::Receiver<Arc<Signature>> {
    let (notify, rx) = oneshot::channel();
    let outcome = state.register_request(notify, threshold);
    assert!(
        outcome.is_continue(),
        "register_notifier should not break the collector"
    );
    rx
}

fn feed_partial_sig(
    state: &mut SignatureCollectorState,
    operator_id: OperatorId,
    signature: Signature,
) {
    let outcome = state.add_partial_signature(operator_id, signature);
    assert!(
        outcome.is_continue(),
        "feed_partial_sig should not break the collector"
    );
}

fn expect_signature(rx: &mut oneshot::Receiver<Arc<Signature>>, context: &str) {
    rx.try_recv().expect(context);
}

struct ContributionProofBatchScenario {
    manager: Arc<SignatureCollectorManager<ManualSlotClock>>,
    _exit_signal: async_channel::Sender<()>,
    message_sender: Arc<CapturingMessageSender>,
    metadata: SignatureMetadata,
    validator_pubkey: PublicKeyBytes,
    base_hash: Hash256,
    validator_index: ValidatorIndex,
    signing_roots: [Hash256; 3],
    shares: Vec<(OperatorId, SecretKey)>,
}

impl ContributionProofBatchScenario {
    fn new() -> Self {
        let message_sender = Arc::new(CapturingMessageSender::default());
        let (manager, exit_signal) = new_test_manager(Arc::clone(&message_sender));
        let validator_pubkey = PublicKeyBytes::empty();
        let validator_index = ValidatorIndex(42);
        let signing_roots = [
            Hash256::repeat_byte(0x01),
            Hash256::repeat_byte(0x02),
            Hash256::repeat_byte(0x03),
        ];
        let metadata = SignatureMetadata {
            kind: PartialSignatureKind::ContributionProofs,
            role: Role::SyncCommittee,
            threshold: THRESHOLD,
            slot: TEST_SLOT,
            committee_id: CommitteeId::default(),
        };

        Self {
            manager,
            _exit_signal: exit_signal,
            message_sender,
            metadata,
            validator_pubkey,
            base_hash: Hash256::repeat_byte(0xAA),
            validator_index,
            signing_roots,
            shares: split_random_master(),
        }
    }

    async fn sign_subnet_0_selection_root(&self) {
        self.sign_subnet_selection_root(0).await;
    }

    async fn retry_subnet_0_selection_root(&self) {
        self.sign_subnet_selection_root(0).await;
    }

    async fn sign_subnet_1_selection_root(&self) {
        self.sign_subnet_selection_root(1).await;
    }

    async fn sign_subnet_2_selection_root(&self) {
        self.sign_subnet_selection_root(2).await;
    }

    async fn retry_subnet_2_selection_root(&self) {
        self.sign_subnet_selection_root(2).await;
    }

    async fn complete_batch(&self) {
        self.sign_subnet_0_selection_root().await;
        self.sign_subnet_1_selection_root().await;
        self.sign_subnet_2_selection_root().await;
    }

    async fn sign_subnet_selection_root(&self, subnet_position: usize) {
        let signing_root = self.signing_roots[subnet_position];
        let manager_for_collect = Arc::clone(&self.manager);
        let slot = self.metadata.slot;
        let metadata = self.metadata.clone();
        let validator_pubkey = self.validator_pubkey;
        let base_hash = self.base_hash;
        let validator_index = self.validator_index;
        let expected_batch_size = self.signing_roots.len();
        let local_share = self.shares[0].1.clone();
        let collect = async move {
            manager_for_collect
                .sign_and_collect(
                    metadata,
                    SignatureRequester::SingleValidatorBatch {
                        pubkey: validator_pubkey,
                        validator_partial_signature_batch_size: expected_batch_size,
                        base_hash,
                    },
                    ValidatorSigningData {
                        root: signing_root,
                        index: validator_index,
                        share: Some(local_share),
                    },
                )
                .await
        };

        for (operator_id, share) in &self.shares[1..THRESHOLD as usize] {
            self.manager
                .receive_partial_signature(
                    PartialSignatureMessage {
                        partial_signature: share.sign(signing_root),
                        signing_root,
                        signer: *operator_id,
                        validator_index,
                    },
                    slot,
                )
                .expect("remote share should be queued");
        }

        collect.await.expect("signature should be collected");
    }

    fn assert_no_envelope_sent(&self, reason: &str) {
        assert_eq!(self.message_sender.messages().len(), 0, "{reason}");
    }

    fn assert_one_envelope_sent(&self, reason: &str) {
        assert_eq!(self.message_sender.messages().len(), 1, "{reason}");
    }

    fn assert_completed_batch_is_retained(&self) {
        let batch = self
            .manager
            .partial_signature_batches
            .get(&(self.base_hash, self.duty_executor()))
            .expect("completed batch should be retained until slot cleanup");

        assert!(batch.completed, "batch should be marked complete");
        assert!(
            batch.batched_validator_partial_signatures.is_empty(),
            "sent signatures should be drained from completed batch"
        );
        assert_eq!(
            batch.seen_validator_partial_signature_keys.len(),
            self.signing_roots.len()
        );
    }

    fn assert_sent_envelope_contains_all_subnet_roots(&self) {
        let sent_messages = self.message_sender.messages();
        let sent_message = sent_messages
            .first()
            .expect("completed batch should send one message");
        assert_eq!(sent_messages.len(), 1);
        assert!(sent_message.full_data.is_empty());

        let duty_executor = self.duty_executor();
        let ssv_message = &sent_message.ssv_message;
        assert_eq!(ssv_message.msg_type(), &MsgType::SSVPartialSignatureMsgType);
        assert_eq!(
            ssv_message.msg_id(),
            &MessageId::new(&DomainType::default(), Role::SyncCommittee, &duty_executor)
        );

        let partial_signature_messages =
            PartialSignatureMessages::from_ssz_bytes(ssv_message.data())
                .expect("partial signature message should decode");
        assert_eq!(
            partial_signature_messages.kind,
            PartialSignatureKind::ContributionProofs
        );
        assert_eq!(partial_signature_messages.slot, TEST_SLOT);
        assert_eq!(
            partial_signature_messages.messages.len(),
            self.signing_roots.len()
        );

        let actual_roots = partial_signature_messages
            .messages
            .iter()
            .map(|message| message.signing_root)
            .collect::<Vec<_>>();
        assert_eq!(actual_roots, self.signing_roots);

        for message in partial_signature_messages.messages {
            assert_eq!(message.signer, TEST_OPERATOR_ID);
            assert_eq!(message.validator_index, self.validator_index);
        }
    }

    fn duty_executor(&self) -> DutyExecutor {
        DutyExecutor::Validator(self.validator_pubkey)
    }
}

/// Regression coverage for pre-Boole sync contribution proofs. A validator can be assigned to
/// multiple sync subnets in one slot, so Anchor must collect those distinct signing roots into one
/// validator-level `ContributionProofs` envelope.
#[tokio::test]
async fn single_validator_batch_waits_for_three_unique_roots_before_sending() {
    let scenario = ContributionProofBatchScenario::new();

    scenario.sign_subnet_0_selection_root().await;
    scenario.assert_no_envelope_sent("one of three contribution-proof roots is not enough to send");

    scenario.sign_subnet_1_selection_root().await;
    scenario.assert_no_envelope_sent(
        "two of three contribution-proof roots are still not enough to send",
    );

    scenario.sign_subnet_2_selection_root().await;
    scenario
        .assert_one_envelope_sent("the third unique contribution-proof root completes the batch");
}

#[tokio::test]
async fn single_validator_batch_ignores_duplicate_roots_before_completion() {
    let scenario = ContributionProofBatchScenario::new();

    scenario.sign_subnet_0_selection_root().await;
    scenario.retry_subnet_0_selection_root().await;
    scenario.sign_subnet_1_selection_root().await;
    scenario.assert_no_envelope_sent("duplicate roots must not count toward batch readiness");

    scenario.sign_subnet_2_selection_root().await;
    scenario.assert_one_envelope_sent("the batch should complete after the third unique root");
}

#[tokio::test]
async fn completed_single_validator_batch_ignores_retried_roots() {
    let scenario = ContributionProofBatchScenario::new();

    scenario.complete_batch().await;
    scenario.retry_subnet_2_selection_root().await;
    scenario.assert_one_envelope_sent("a retry after completion must not send another envelope");

    scenario.assert_completed_batch_is_retained();
}

#[tokio::test]
async fn single_validator_batch_envelope_contains_all_contribution_proof_roots() {
    let scenario = ContributionProofBatchScenario::new();

    scenario.complete_batch().await;

    scenario.assert_sent_envelope_contains_all_subnet_roots();
}

/// Once `THRESHOLD` valid partial signatures have been fed in, the state
/// reconstructs the master signature and delivers it to a registered notifier.
#[test]
fn state_processes_full_quorum_and_notifies() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    let mut result_rx = register_notifier(&mut state, THRESHOLD);

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    expect_signature(
        &mut result_rx,
        "Notifier should receive the reconstructed signature",
    );
}

/// Two `RegisterNotifier` messages disagree on the threshold; the state must
/// `Break`. In production, the recv loop drops the state on `Break`, which
/// drops every queued notifier and surfaces `RecvError` to the callers. The
/// test models that lifetime explicitly via `drop(state)`.
#[test]
fn state_breaks_on_conflicting_thresholds() {
    let mut state = SignatureCollectorState::default();
    let _first_rx = register_notifier(&mut state, THRESHOLD);

    let (second_notify, mut second_rx) = oneshot::channel();
    let outcome = state.register_request(second_notify, THRESHOLD + 1);

    assert!(
        outcome.is_break(),
        "State should Break when a second notifier disagrees on threshold"
    );

    drop(state);

    assert!(
        matches!(
            second_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ),
        "Conflicting notifier sender should be dropped, surfacing RecvError"
    );
}

/// A notifier that registers after reconstruction has already completed should
/// receive the cached signature immediately rather than waiting on a fresh
/// quorum.
#[test]
fn state_delivers_cached_signature_to_late_registrant() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    // Reach quorum on the first registrant.
    let mut first_rx = register_notifier(&mut state, THRESHOLD);
    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }
    expect_signature(
        &mut first_rx,
        "First registrant should receive the reconstructed signature",
    );

    // Late registrant arrives after `full_signature` is set.
    let mut late_rx = register_notifier(&mut state, THRESHOLD);
    expect_signature(
        &mut late_rx,
        "Late registrant should receive the cached signature immediately",
    );
}

/// Multiple notifiers registered before quorum should all be notified on a
/// single reconstruction (the `Vec<oneshot::Sender>` fan-out).
#[test]
fn state_notifies_all_registrants_on_reconstruction() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    let mut rx_a = register_notifier(&mut state, THRESHOLD);
    let mut rx_b = register_notifier(&mut state, THRESHOLD);
    let mut rx_c = register_notifier(&mut state, THRESHOLD);

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    expect_signature(&mut rx_a, "registrant a should be notified");
    expect_signature(&mut rx_b, "registrant b should be notified");
    expect_signature(&mut rx_c, "registrant c should be notified");
}

/// Partial signatures that arrive before any `RegisterNotifier` is buffered.
/// Reconstruction triggers when a notifier and the threshold arrives.
#[test]
fn state_buffers_shares_arriving_before_first_notifier_register() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    assert!(
        state.full_signature.is_none(),
        "State should not reconstruct without a threshold"
    );

    let mut result_rx = register_notifier(&mut state, THRESHOLD);
    expect_signature(
        &mut result_rx,
        "Notifier should receive the signature reconstructed from buffered shares",
    );
}

/// Partial signatures that arrive after reconstruction are silently dropped:
/// no panic, no Break, no re-buffering of the late share. This is the common
/// case in production: slow operators' shares routinely arrive after a fast
/// majority has already reconstructed the signature.
#[test]
fn state_drops_partial_signatures_after_reconstruction() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    let mut rx = register_notifier(&mut state, THRESHOLD);
    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }
    expect_signature(&mut rx, "first registrant should be notified");

    let (late_op, late_sk) = &shares[THRESHOLD as usize];
    feed_partial_sig(&mut state, *late_op, late_sk.sign(SIGNING_ROOT));

    assert!(state.full_signature.is_some(), "cached signature persists");
    assert!(
        state.signature_share.is_empty(),
        "post-reconstruction shares are not buffered"
    );
}
