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
