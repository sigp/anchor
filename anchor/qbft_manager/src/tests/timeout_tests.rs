use super::*;
use crate::{LEGACY_PROPOSER_ROUND_TIMEOUT, PROPOSER_ROUND_TIMEOUT};

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
    // Start the instance 4 seconds in the future (simulating slot timing).
    let total_time = run_from_now_until_timed_out(
        |now| TimeoutMode::Relative {
            current_round_start_time: now + Duration::from_secs(4),
            round_timeout: PROPOSER_ROUND_TIMEOUT,
        },
        3,
    )
    .await;

    // Relative mode waits until current_round_start_time, then runs 3 single 1.5 second rounds.
    // SlotTime would measure cumulative 2 second deadlines from the same instant and time out
    // at 4 + 2 + 2 + 2 = 10 seconds instead.
    assert_eq!(total_time, Duration::from_millis(8_500));
}

/// Test that the `round_timeout` carried by `TimeoutMode::Relative` is the budget the instance
/// timer actually uses, so the `--legacy-proposer-round-timeout` rollback restores 2 second
/// rounds rather than only changing a config value.
#[tokio::test(start_paused = true)]
async fn test_relative_mode_uses_configured_round_timeout() {
    for (round_timeout, expected) in [
        (PROPOSER_ROUND_TIMEOUT, Duration::from_secs(3)),
        (LEGACY_PROPOSER_ROUND_TIMEOUT, Duration::from_secs(4)),
    ] {
        let elapsed = run_from_now_until_timed_out(
            |now| TimeoutMode::Relative {
                current_round_start_time: now,
                round_timeout,
            },
            2,
        )
        .await;

        assert_eq!(elapsed, expected, "round_timeout {round_timeout:?}");
    }
}

/// Run a single instance, building its timeout mode from the current instant, and return the
/// elapsed time until it reports `Completed::TimedOut` after `max_rounds` rounds.
async fn run_from_now_until_timed_out(
    timeout_mode: impl FnOnce(Instant) -> TimeoutMode,
    max_rounds: usize,
) -> Duration {
    let (sender_tx, _sender_rx) = unbounded_channel();
    let (message_tx, message_rx) = unbounded_channel();
    let (result_tx, result_rx) = oneshot::channel();
    let message_sender = MockMessageSender::new(sender_tx, OperatorId(1));
    let _handle = tokio::spawn(qbft_instance::<BeaconVote>(
        message_rx,
        Arc::new(message_sender),
    ));

    let now = Instant::now();

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
                timeout_mode: timeout_mode(now),
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

    assert!(matches!(result_rx.await, Ok(Completed::TimedOut)));
    Instant::now() - now
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
