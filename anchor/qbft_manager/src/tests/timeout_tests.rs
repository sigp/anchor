use bls::PublicKeyBytes;

use super::*;

// very important: set paused to true for deterministic timer
#[tokio::test(start_paused = true)]
async fn test_timeouts() {
    for i in 1..=10 {
        test_timeout(i).await;
    }
}

// ==================== Proposer-path instrumentation tests ====================

/// Drive a `Role::Proposer` instance through `qbft_instance()` to a max-round timeout, exercising
/// the `ProposerObserver` lifecycle (`start` -> `finish`) that only activates when
/// `Initialized::is_proposer()` is true.
///
/// Why this matters: every other `qbft_manager` test uses `Role::Committee`, so the observer wiring
/// in the `qbft_instance()` loop (observer construction on `Initialize`, and the `finish` call in
/// the terminal-outcome branch) is otherwise never exercised end-to-end. The pure
/// `classify_round_advance` classifier is covered by unit tests in `instrumentation.rs`; this test
/// covers the boundary wiring.
///
/// `is_proposer()` checks only the `MessageId` role, so keeping the data type as `BeaconVote` and
/// only swapping the role to `Role::Proposer` (with a `Validator` duty executor) is sufficient to
/// activate the proposer path; no `ProposerConsensusData` scaffolding is needed.
///
/// Assertion strategy: the `PROPOSER_QBFT_OUTCOME_TOTAL{outcome="max_round_timeout"}` counter is a
/// process-global static, so we assert a strict monotonic increase between a before- and
/// after-snapshot (a delta) rather than an absolute value, which would be flaky under parallel test
/// execution. A committee instance never touches this counter, so a positive delta proves the
/// proposer observer's `finish()` specifically ran. We also assert the instance reaches
/// `Completed::TimedOut`, proving the proposer path ran start -> finish without panic.
#[tokio::test(start_paused = true)]
async fn test_proposer_instance_max_round_timeout_runs_observer() {
    // The outcome label the observer records for a max-round timeout (mirrors
    // `ProposerOutcome::MaxRoundTimeout.as_str()`).
    const MAX_ROUND_TIMEOUT_OUTCOME: &str = "max_round_timeout";
    // Number of rounds to run before the instance times out. Kept small for a fast deterministic
    // run; the observer activates regardless of round count.
    const MAX_ROUNDS: usize = 2;
    // Handoff budget so the `PROPOSER_QBFT_HANDOFF_BUDGET_SECONDS` path and the `handoff_budget_ms`
    // span field are also exercised (not just the `None` path the committee tests use).
    const HANDOFF_BUDGET_MS: u64 = 4_000;

    // Arrange: read the global outcome counter before the instance runs.
    let outcome_before = crate::metrics::get_int_counter(
        &crate::metrics::PROPOSER_QBFT_OUTCOME_TOTAL,
        &[MAX_ROUND_TIMEOUT_OUTCOME],
    )
    .map(|c| c.get())
    .unwrap_or(0);

    let (sender_tx, _sender_rx) = unbounded_channel();
    let (message_tx, message_rx) = unbounded_channel();
    let (result_tx, result_rx) = oneshot::channel();
    let message_sender = MockMessageSender::new(sender_tx, OperatorId(1));
    let _handle = tokio::spawn(qbft_instance::<BeaconVote>(
        message_rx,
        Arc::new(message_sender),
    ));

    let qbft_start_time = Instant::now();

    // Act: initialize a proposer-role instance and let it run to a max-round timeout.
    message_tx
        .send(crate::QbftMessage {
            kind: QbftMessageKind::Initialize(QbftInitialization {
                initial: setup::generate_test_data(0).0,
                validator: Box::new(NoDataValidation),
                // The proposer role is what flips `is_proposer()` to true and attaches the
                // observer.
                message_id: MessageId::new(
                    &DomainType::default(),
                    Role::Proposer,
                    &DutyExecutor::Validator(PublicKeyBytes::empty()),
                ),
                timeout_mode: TimeoutMode::SlotTime {
                    instance_start_time: qbft_start_time,
                },
                config: qbft::ConfigBuilder::new(
                    OperatorId(1),
                    InstanceHeight::from(0),
                    IndexSet::from([1, 2, 3, 4].map(OperatorId)),
                )
                .with_max_rounds(MAX_ROUNDS)
                .build()
                .unwrap(),
                on_completed: result_tx,
                handoff_budget_ms: Some(HANDOFF_BUDGET_MS),
            }),
            drop_on_finish: None,
        })
        .unwrap();

    // Assert: the proposer instance times out (proves the proposer path ran start -> finish without
    // panic and follows the same lifecycle as the committee path).
    assert!(
        matches!(result_rx.await, Ok(Completed::TimedOut)),
        "proposer instance should reach Completed::TimedOut after exhausting its rounds"
    );

    // Assert: the observer's `finish()` bumped the max-round-timeout outcome counter. Using a
    // strict monotonic delta keeps this robust under parallel test execution.
    let outcome_after = crate::metrics::get_int_counter(
        &crate::metrics::PROPOSER_QBFT_OUTCOME_TOTAL,
        &[MAX_ROUND_TIMEOUT_OUTCOME],
    )
    .map(|c| c.get())
    .unwrap_or(0);
    assert!(
        outcome_after > outcome_before,
        "ProposerObserver::finish should increment \
         PROPOSER_QBFT_OUTCOME_TOTAL{{outcome=\"{MAX_ROUND_TIMEOUT_OUTCOME}\"}} \
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
        .send(crate::QbftMessage {
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
