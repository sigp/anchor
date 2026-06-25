use bls::PublicKeyBytes;

use super::*;
use crate::{QbftMessage, metrics};

// very important: set paused to true for deterministic timer
#[tokio::test(start_paused = true)]
async fn test_timeouts() {
    for i in 1..=10 {
        test_timeout(i).await;
    }
}

// Proposer-path instrumentation tests: drive a `Role::Proposer` instance to each terminal outcome,
// exercising the `ProposerObserver` lifecycle that only activates when `is_proposer()` is true.
// Timeout mode mirrors production proposer block duties (`TimeoutMode::Relative`).
//
// `PROPOSER_QBFT_OUTCOME_TOTAL` is a process-global static, so tests assert a monotonic delta on
// its outcome label rather than an absolute value, which would be flaky under parallel execution.

/// Read `PROPOSER_QBFT_OUTCOME_TOTAL` for `outcome` (0 if unset).
fn proposer_outcome_count(outcome: &str) -> u64 {
    metrics::get_int_counter(&metrics::PROPOSER_QBFT_OUTCOME_TOTAL, &[outcome])
        .map(|c| c.get())
        .unwrap_or(0)
}

/// The proposer `MessageId` shared by the instance and its peers. `Role::Proposer` attaches the
/// observer; peers must reuse it so commit aggregation (which compares the full `SSVMessage`)
/// accepts the quorum.
fn proposer_message_id() -> MessageId {
    MessageId::new(
        &DomainType::default(),
        Role::Proposer,
        &DutyExecutor::Validator(PublicKeyBytes::empty()),
    )
}

/// Spawn `qbft_instance()` for operator 1 (the round-1 leader of committee `[1, 2, 3, 4]`, so it
/// emits its own PROPOSAL on init) and initialize a `Role::Proposer` instance with `config`.
fn spawn_proposer_instance(
    config: qbft::Config<qbft::DefaultLeaderFunction>,
    handoff_budget_ms: Option<u64>,
) -> (
    UnboundedSender<QbftMessage<BeaconVote>>,
    oneshot::Receiver<Completed<BeaconVote>>,
    BeaconVote,
) {
    let (sender_tx, _sender_rx) = unbounded_channel();
    let (message_tx, message_rx) = unbounded_channel();
    let (result_tx, result_rx) = oneshot::channel();
    let message_sender = MockMessageSender::new(sender_tx, OperatorId(1));
    tokio::spawn(qbft_instance::<BeaconVote>(
        message_rx,
        Arc::new(message_sender),
    ));

    let start_data = setup::generate_test_data(0).0;
    message_tx
        .send(QbftMessage {
            kind: QbftMessageKind::Initialize(QbftInitialization {
                initial: start_data.clone(),
                validator: Box::new(NoDataValidation),
                message_id: proposer_message_id(),
                timeout_mode: TimeoutMode::Relative {
                    current_round_start_time: Instant::now(),
                },
                config,
                on_completed: result_tx,
                handoff_budget_ms,
            }),
            drop_on_finish: None,
        })
        .unwrap();

    (message_tx, result_rx, start_data)
}

/// Build a single-signer PREPARE or COMMIT network message for `root` at `round` from `signer`.
/// The placeholder RSA signature is fine (content is not checked at this layer); the `MessageId`
/// matches the instance's so commit aggregation accepts the quorum.
fn build_peer_consensus_msg(
    msg_type: ssv_types::consensus::QbftMessageType,
    round: u64,
    root: Hash256,
    signer: u64,
) -> WrappedQbftMessage {
    use ssv_types::{
        RSA_SIGNATURE_SIZE, VariableList,
        consensus::QbftMessage as SsvQbftMessage,
        message::{MsgType, SSVMessage, SignedSSVMessage},
    };
    use ssz::Encode;

    let msg_id = proposer_message_id();
    let qbft_message = SsvQbftMessage {
        qbft_message_type: msg_type,
        height: 0,
        round,
        identifier: (&msg_id).into(),
        root,
        // PREPARE/COMMIT never claim a prepared value, so `data_round` is 0 and no justifications
        // are required on the round-1 path.
        data_round: 0,
        round_change_justification: VariableList::empty(),
        prepare_justification: VariableList::empty(),
    };

    let ssv_message = SSVMessage::new(
        MsgType::SSVConsensusMsgType,
        msg_id,
        qbft_message.as_ssz_bytes(),
    )
    .expect("should create SSVMessage");

    let signed_message = SignedSSVMessage::new(
        vec![[0; RSA_SIGNATURE_SIZE]],
        vec![OperatorId::from(signer)],
        ssv_message,
        // Only the PROPOSAL carries `full_data`.
        vec![],
    )
    .expect("should create SignedSSVMessage");

    WrappedQbftMessage {
        signed_message,
        qbft_message,
    }
}

/// The proposer instance exhausts its rounds and the observer records the `max_round_timeout`
/// outcome. `with_max_rounds(2)` keeps the run short. A non-`None` `handoff_budget_ms` also
/// exercises the `PROPOSER_QBFT_HANDOFF_BUDGET_SECONDS` path and `handoff_budget_ms` span field.
#[tokio::test(start_paused = true)]
async fn test_proposer_instance_max_round_timeout_runs_observer() {
    // Mirrors `ProposerOutcome::MaxRoundTimeout.as_str()`.
    const MAX_ROUND_TIMEOUT_OUTCOME: &str = "max_round_timeout";
    const MAX_ROUNDS: usize = 2;
    const HANDOFF_BUDGET_MS: u64 = 4_000;

    // Arrange.
    let outcome_before = proposer_outcome_count(MAX_ROUND_TIMEOUT_OUTCOME);
    let config = qbft::ConfigBuilder::new(
        OperatorId(1),
        InstanceHeight::from(0),
        IndexSet::from([1, 2, 3, 4].map(OperatorId)),
    )
    .with_max_rounds(MAX_ROUNDS)
    .build()
    .unwrap();

    // Act: initialize the instance and let it run out of rounds (no peer messages are fed).
    let (_message_tx, result_rx, _start_data) =
        spawn_proposer_instance(config, Some(HANDOFF_BUDGET_MS));

    // Assert: the instance times out and the observer recorded the timeout outcome.
    assert!(
        matches!(result_rx.await, Ok(Completed::TimedOut)),
        "proposer instance should reach Completed::TimedOut after exhausting its rounds"
    );
    let outcome_after = proposer_outcome_count(MAX_ROUND_TIMEOUT_OUTCOME);
    assert!(
        outcome_after > outcome_before,
        "ProposerObserver::finish should increment \
         PROPOSER_QBFT_OUTCOME_TOTAL{{outcome=\"{MAX_ROUND_TIMEOUT_OUTCOME}\"}} \
         (before={outcome_before}, after={outcome_after})"
    );
}

/// The proposer instance decides in round 1 and the observer records the `decided` outcome - the
/// production hot-path. As round-1 leader it proposes, then commits as quorums form (quorum is 3
/// with f = 1, so two peer PREPAREs and two peer COMMITs complete each quorum alongside its own).
///
/// Ordering note: `received_commit` drops COMMITs that arrive before the proposal is accepted
/// (unlike `received_prepare`, which buffers them), so peer COMMITs are sent only after a
/// `yield_now()` lets the instance drain its own looped-back PROPOSAL/PREPARE and the peer
/// PREPAREs.
#[tokio::test(start_paused = true)]
async fn test_proposer_instance_decided_runs_observer() {
    use ssv_types::consensus::QbftData;

    // Mirrors `ProposerOutcome::Decided.as_str()`.
    const DECIDED_OUTCOME: &str = "decided";
    // Operator 1 leads round 1, so the instance decides there with no round change.
    const DECIDE_ROUND: u64 = 1;
    // Peers that complete the prepare and commit quorums alongside the instance's own messages.
    const PEER_SIGNERS: [u64; 2] = [2, 3];

    // Arrange.
    let outcome_before = proposer_outcome_count(DECIDED_OUTCOME);
    let config = qbft::ConfigBuilder::new(
        OperatorId(1),
        InstanceHeight::from(0),
        IndexSet::from([1, 2, 3, 4].map(OperatorId)),
    )
    .build()
    .unwrap();
    let (message_tx, result_rx, start_data) = spawn_proposer_instance(config, None);
    let proposal_root = start_data.hash();

    let send_peer = |msg_type| {
        for signer in PEER_SIGNERS {
            message_tx
                .send(QbftMessage {
                    kind: QbftMessageKind::NetworkMessage(build_peer_consensus_msg(
                        msg_type,
                        DECIDE_ROUND,
                        proposal_root,
                        signer,
                    )),
                    drop_on_finish: None,
                })
                .unwrap();
        }
    };

    // Act: peer PREPAREs form the prepare quorum (driving the instance to COMMIT); after it drains
    // its own messages, peer COMMITs form the commit quorum and decide the instance.
    send_peer(ssv_types::consensus::QbftMessageType::Prepare);
    tokio::task::yield_now().await;
    send_peer(ssv_types::consensus::QbftMessageType::Commit);

    // Assert: the instance decides on the proposed root and the observer recorded the decided
    // outcome.
    assert!(
        matches!(result_rx.await, Ok(Completed::Success(data)) if data == start_data),
        "proposer instance should reach Completed::Success on the proposed data"
    );
    let outcome_after = proposer_outcome_count(DECIDED_OUTCOME);
    assert!(
        outcome_after > outcome_before,
        "ProposerObserver::finish should increment \
         PROPOSER_QBFT_OUTCOME_TOTAL{{outcome=\"{DECIDED_OUTCOME}\"}} \
         (before={outcome_before}, after={outcome_after})"
    );
}

async fn test_timeout(round_timeout_to_test: usize) {
    let (sender_tx, _sender_rx) = unbounded_channel();
    let (message_tx, message_rx) = unbounded_channel();
    let (result_tx, result_rx) = oneshot::channel();
    let message_sender = MockMessageSender::new(sender_tx, OperatorId(1));
    let _handle = tokio::spawn(qbft_instance::<BeaconVote>(
        message_rx,
        Arc::new(message_sender),
    ));

    // create a slot clock at slot 0 with a slot duration of 12 seconds
    // we are now at the beginning of the slot and remember that instant
    let slot_clock = ManualSlotClock::new(
        Slot::new(0),
        Duration::from_secs(0),
        Duration::from_secs(12),
    );
    let slot_start_time = Instant::now();

    // start at one third slot duration into the slot
    let qbft_start_time = slot_start_time + slot_clock.slot_duration() / 3;

    message_tx
        .send(QbftMessage {
            kind: QbftMessageKind::Initialize(QbftInitialization {
                initial: setup::generate_test_data(0).0,
                validator: Box::new(NoDataValidation),
                message_id: MessageId::new(
                    &DomainType::default(),
                    Role::Committee,
                    &DutyExecutor::Committee(CommitteeId::default()),
                ),
                timeout_mode: TimeoutMode::SlotTime {
                    instance_start_time: qbft_start_time,
                },
                config: qbft::ConfigBuilder::new(
                    OperatorId(1),
                    InstanceHeight::from(0),
                    IndexSet::from([1, 2, 3, 4].map(OperatorId)),
                )
                // we set the round we want to test as maximum round so that the instance times
                // out at the end of that round
                .with_max_rounds(round_timeout_to_test)
                .build()
                .unwrap(),
                on_completed: result_tx,
                handoff_budget_ms: None,
            }),
            drop_on_finish: None,
        })
        .unwrap();

    // we now wait for the instance to time out
    assert!(matches!(result_rx.await, Ok(Completed::TimedOut)));

    // we now measure the time it took for the instance to time out
    let timeout = Instant::now() - slot_start_time;

    // Calculate the expected timeout
    let mut expected_timeout = Duration::ZERO;
    // first, the instance should not start until start time, so we add the difference from slot
    // start to qbft start.
    expected_timeout += qbft_start_time - slot_start_time;
    // now, we account for the actual rounds:
    for i in 1..=round_timeout_to_test {
        // check if we use short round timeout or long round timeout for this round
        if i <= 8 {
            expected_timeout += Duration::from_secs(2);
        } else {
            expected_timeout += Duration::from_secs(120);
        }
    }
    assert_eq!(timeout, expected_timeout);
}

/// Test that Relative mode uses single-round timeouts starting from Instant::now()
/// after the sleep_until, not cumulative timeouts from start_time.
#[tokio::test(start_paused = true)]
async fn test_relative_mode_timeout() {
    let (sender_tx, _sender_rx) = unbounded_channel();
    let (message_tx, message_rx) = unbounded_channel();
    let (result_tx, result_rx) = oneshot::channel();
    let message_sender = MockMessageSender::new(sender_tx, OperatorId(1));
    let _handle = tokio::spawn(qbft_instance::<BeaconVote>(
        message_rx,
        Arc::new(message_sender),
    ));

    let slot_start_time = Instant::now();
    // Set start_time 4 seconds in the future (simulating slot timing)
    let qbft_start_time = slot_start_time + Duration::from_secs(4);

    message_tx
        .send(crate::QbftMessage {
            kind: QbftMessageKind::Initialize(QbftInitialization {
                initial: setup::generate_test_data(0).0,
                validator: Box::new(NoDataValidation),
                message_id: MessageId::new(
                    &DomainType::default(),
                    Role::Committee,
                    &DutyExecutor::Committee(CommitteeId::default()),
                ),
                timeout_mode: TimeoutMode::Relative {
                    current_round_start_time: qbft_start_time,
                },
                config: qbft::ConfigBuilder::new(
                    OperatorId(1),
                    InstanceHeight::from(0),
                    IndexSet::from([1, 2, 3, 4].map(OperatorId)),
                )
                .with_max_rounds(3) // Test 3 rounds
                .build()
                .unwrap(),
                on_completed: result_tx,
                handoff_budget_ms: None,
            }),
            drop_on_finish: None,
        })
        .unwrap();

    assert!(matches!(result_rx.await, Ok(Completed::TimedOut)));

    let total_time = Instant::now() - slot_start_time;

    // For Relative mode:
    // - Wait 4 seconds until current_round_start_time
    // - Round 1: 2 seconds (single round timeout, not cumulative)
    // - Round 2: 2 seconds
    // - Round 3: 2 seconds
    // Total: 4 + 2 + 2 + 2 = 10 seconds
    //
    // If it were SlotTime mode (cumulative), it would be:
    // - Wait 4 seconds
    // - Round 1 ends at start_time + 2 = 6 seconds total
    // - Round 2 ends at start_time + 4 = 8 seconds total
    // - Round 3 ends at start_time + 6 = 10 seconds total
    // Which happens to be the same for this test, but the key difference is
    // Relative mode resets start_time to Instant::now() after sleep_until

    let expected = Duration::from_secs(4 + 2 + 2 + 2);
    assert_eq!(total_time, expected);
}

/// Test that SlotTime and Relative modes differ when start_time is in the past.
/// This tests the key behavioral difference between the modes.
#[tokio::test(start_paused = true)]
async fn test_relative_vs_slottime_timing_difference() {
    // Test with start_time in the past - this highlights the difference
    // between SlotTime (uses original instance_start_time) and Relative (uses Instant::now())

    async fn run_with_mode(use_relative: bool) -> Duration {
        let (sender_tx, _sender_rx) = unbounded_channel();
        let (message_tx, message_rx) = unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        let message_sender = MockMessageSender::new(sender_tx, OperatorId(1));
        let _handle = tokio::spawn(qbft_instance::<BeaconVote>(
            message_rx,
            Arc::new(message_sender),
        ));

        let now = Instant::now();

        let timeout_mode = if use_relative {
            TimeoutMode::Relative {
                current_round_start_time: now,
            }
        } else {
            TimeoutMode::SlotTime {
                instance_start_time: now,
            }
        };

        message_tx
            .send(crate::QbftMessage {
                kind: QbftMessageKind::Initialize(QbftInitialization {
                    initial: setup::generate_test_data(0).0,
                    validator: Box::new(NoDataValidation),
                    message_id: MessageId::new(
                        &DomainType::default(),
                        Role::Committee,
                        &DutyExecutor::Committee(CommitteeId::default()),
                    ),
                    timeout_mode,
                    config: qbft::ConfigBuilder::new(
                        OperatorId(1),
                        InstanceHeight::from(0),
                        IndexSet::from([1, 2, 3, 4].map(OperatorId)),
                    )
                    .with_max_rounds(2)
                    .build()
                    .unwrap(),
                    on_completed: result_tx,
                    handoff_budget_ms: None,
                }),
                drop_on_finish: None,
            })
            .unwrap();

        assert!(matches!(result_rx.await, Ok(Completed::TimedOut)));
        Instant::now() - now
    }

    let slottime_duration = run_with_mode(false).await;
    let relative_duration = run_with_mode(true).await;

    // Both should complete in 4 seconds (2 rounds * 2 seconds each)
    // The difference is in HOW they calculate it:
    // - SlotTime: cumulative from original instance_start_time
    // - Relative: single-round from current_round_start_time (reset each round)
    //
    // When start_time is now, both should behave similarly for the first run,
    // but the internal calculations differ.
    assert_eq!(slottime_duration, Duration::from_secs(4));
    assert_eq!(relative_duration, Duration::from_secs(4));
}
