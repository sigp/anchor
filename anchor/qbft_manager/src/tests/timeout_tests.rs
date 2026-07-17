use super::*;

// very important: set paused to true for deterministic timer
#[tokio::test(start_paused = true)]
async fn test_timeouts() {
    for i in 1..=10 {
        test_timeout(i).await;
    }
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
                    round_deadline_origin: qbft_start_time,
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
    // The instance starts immediately, but the round deadlines are measured from
    // `qbft_start_time`, so we add the difference from slot start to qbft start.
    expected_timeout += qbft_start_time - slot_start_time;
    // now, we account for the actual rounds:
    expected_timeout += cumulative_timeout(round_timeout_to_test);
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
    // If it were SlotTime mode, the instance would start immediately, but the cumulative
    // deadlines are measured from the same instant:
    // - Round 1 ends at round_deadline_origin + 2 = 6 seconds total
    // - Round 2 ends at round_deadline_origin + 4 = 8 seconds total
    // - Round 3 ends at round_deadline_origin + 6 = 10 seconds total
    // Which happens to be the same for this test, but the key difference is
    // Relative mode sleeps until the round start and resets it to Instant::now()

    let expected = Duration::from_secs(4 + 2 + 2 + 2);
    assert_eq!(total_time, expected);
}

/// Test that SlotTime and Relative modes compute their round deadlines differently.
/// SlotTime measures cumulative deadlines from `round_deadline_origin`, while Relative restarts
/// each round to `Instant::now()`.
#[tokio::test(start_paused = true)]
async fn test_relative_vs_slottime_timing_difference() {
    // With the origin at `now`, both modes complete at the same instant, but via
    // different calculations (cumulative-from-origin vs per-round-from-now)

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
                round_deadline_origin: now,
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
    // - SlotTime: cumulative from the fixed round_deadline_origin
    // - Relative: single-round from current_round_start_time (reset each round)
    //
    // With the origin at now, both arrive at the same deadlines, but the internal
    // calculations differ.
    assert_eq!(slottime_duration, Duration::from_secs(4));
    assert_eq!(relative_duration, Duration::from_secs(4));
}

/// Test that `SlotTime` round deadlines are a pure function of `round_deadline_origin`: an instance
/// initialized before the origin must time out at the exact same instant as one initialized
/// at the origin.
#[tokio::test(start_paused = true)]
async fn test_slottime_round_deadlines_invariant_under_early_init() {
    // Origin 4 seconds after scenario start, matching the one-third-slot offset used in
    // production for a 12 second slot
    const ORIGIN_OFFSET: Duration = Duration::from_secs(4);

    // `max_rounds = 9` crosses from the 2 second quick timeouts into the 120 second slow
    // timeout for round 9
    for max_rounds in [2, 9] {
        // Arrange + Act: run the identical scenario twice, once initializing at scenario
        // start (before the origin) and once initializing exactly at the origin
        let early_init_elapsed =
            run_slottime_scenario(ORIGIN_OFFSET, Duration::ZERO, max_rounds).await;
        let at_origin_elapsed =
            run_slottime_scenario(ORIGIN_OFFSET, ORIGIN_OFFSET, max_rounds).await;

        // Assert: both time out at exactly `round_deadline_origin +
        // cumulative_timeout(max_rounds)`, proving the deadlines depend on the origin, not
        // on initialization time
        let expected = ORIGIN_OFFSET + cumulative_timeout(max_rounds);
        assert_eq!(early_init_elapsed, expected);
        assert_eq!(at_origin_elapsed, expected);
    }
}

/// Test that an instance initialized after `round_deadline_origin` cascades through the already
/// expired rounds immediately and still times out at the origin-based deadline.
#[tokio::test(start_paused = true)]
async fn test_slottime_late_init_cascades_round_changes() {
    // Arrange + Act: origin at scenario start, initialize 5 seconds later. Rounds 1 and 2
    // (deadlines at origin + 2s/4s) are already expired at initialization and fire
    // immediately, leaving only round 3's deadline at origin + 6s.
    let elapsed = run_slottime_scenario(Duration::ZERO, Duration::from_secs(5), 3).await;

    // Assert: the instance times out exactly 1 second after initialization, at the
    // origin-based deadline of round 3
    assert_eq!(elapsed, Duration::from_secs(6));
}

/// Run a single `SlotTime` instance whose `round_deadline_origin` lies `origin_offset` after the
/// scenario start, sending the initialization after `init_delay`. Returns the elapsed time
/// from scenario start until the instance reports `Completed::TimedOut`.
async fn run_slottime_scenario(
    origin_offset: Duration,
    init_delay: Duration,
    max_rounds: usize,
) -> Duration {
    let (sender_tx, mut sender_rx) = unbounded_channel();
    let (message_tx, message_rx) = unbounded_channel();
    let (result_tx, result_rx) = oneshot::channel();
    let message_sender = MockMessageSender::new(sender_tx, OperatorId(1));
    let _handle = tokio::spawn(qbft_instance::<BeaconVote>(
        message_rx,
        Arc::new(message_sender),
    ));

    let scenario_start = Instant::now();
    let round_deadline_origin = scenario_start + origin_offset;

    tokio::time::sleep(init_delay).await;

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
                    round_deadline_origin,
                },
                config: qbft::ConfigBuilder::new(
                    OperatorId(1),
                    InstanceHeight::from(0),
                    IndexSet::from([1, 2, 3, 4].map(OperatorId)),
                )
                .with_max_rounds(max_rounds)
                .build()
                .unwrap(),
                on_completed: result_tx,
            }),
            drop_on_finish: None,
        })
        .unwrap();

    if init_delay < origin_offset {
        // The instance must not wait for the origin: operator 1 is the round 1 leader at
        // instance height 0 under `DefaultLeaderFunction`, so its proposal must go out
        // before the origin. Advance virtual time by 1ms so the instance task runs.
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert!(
            Instant::now() < round_deadline_origin,
            "test setup error: still expected to be before the origin"
        );
        assert!(
            sender_rx.try_recv().is_ok(),
            "round 1 proposal should be emitted before the origin"
        );
    }

    assert!(matches!(result_rx.await, Ok(Completed::TimedOut)));
    Instant::now() - scenario_start
}

/// Cumulative round timeout as implemented in `crate::timeout`: rounds 1 to 8 add 2 seconds
/// each, every round beyond adds 120 seconds.
fn cumulative_timeout(max_rounds: usize) -> Duration {
    let mut total = Duration::ZERO;
    for round in 1..=max_rounds {
        if round <= 8 {
            total += Duration::from_secs(2);
        } else {
            total += Duration::from_secs(120);
        }
    }
    total
}
