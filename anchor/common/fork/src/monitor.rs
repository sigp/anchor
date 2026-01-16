//! Fork transition monitoring.
//!
//! This module provides a standalone task that monitors and logs fork transitions,
//! giving operators visibility into:
//! - Current active fork at startup
//! - Entering the preparation window before a fork
//! - Fork activation when it occurs
//!
//! The monitor exits automatically when all scheduled forks have activated.
//!
//! ## Sleep Strategy
//!
//! Instead of waking up every epoch to check for state changes, the monitor
//! calculates the next interesting event (preparation window start or fork
//! activation) and sleeps directly until that time. This is more efficient
//! and precise than periodic polling.

use std::{sync::Arc, time::Duration};

use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::sync::mpsc;
use tracing::{info, warn};
use types::Epoch;

use crate::{FORK_PREPARATION_EPOCHS, Fork, ForkConfig, ForkSchedule};

/// Fork transition events sent to Network and other components.
///
/// These events tell components when to:
/// - Subscribe to new topics (Preparing)
/// - Unsubscribe from old topics (Activated)
#[derive(Clone, Debug)]
pub enum ForkPhase {
    /// Entering preparation window - subscribe to new topics (dual-subscription).
    Preparing {
        /// Configuration for the upcoming fork.
        upcoming: ForkConfig,
    },
    /// Fork activated - unsubscribe from old topics.
    Activated {
        /// Configuration for the now-active fork.
        current: ForkConfig,
        /// Configuration for the previous fork.
        previous: ForkConfig,
    },
}

/// Sender for fork phase events.
pub type ForkPhaseSender = mpsc::Sender<ForkPhase>;

/// Events emitted by the fork monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkEvent {
    /// Monitor started, reporting current state.
    Started { fork: Fork, epoch: Epoch },
    /// A fork is scheduled for a future epoch.
    Scheduled {
        fork: Fork,
        fork_epoch: Epoch,
        epochs_until: u64,
    },
    /// Entered the preparation window before a fork.
    PreparationStarted {
        fork: Fork,
        current_epoch: Epoch,
        fork_epoch: Epoch,
        epochs_until: u64,
    },
    /// A fork has activated.
    Activated {
        previous_fork: Fork,
        new_fork: Fork,
        epoch: Epoch,
    },
    /// No more forks scheduled, monitor exiting.
    Complete,
}

/// Tracks fork monitor state and emits events on state changes.
pub struct ForkMonitorState {
    fork_schedule: Arc<ForkSchedule>,
    current_fork: Fork,
    next_fork: Option<(Fork, Epoch)>,
    in_preparation: bool,
}

impl ForkMonitorState {
    /// Create a new monitor state and return initial events.
    pub fn new(fork_schedule: Arc<ForkSchedule>, current_epoch: Epoch) -> (Self, Vec<ForkEvent>) {
        let current_fork = fork_schedule.active_fork(current_epoch);
        let next_fork = fork_schedule.next_fork_after(current_epoch);
        let in_preparation = next_fork
            .map(|(fork, _)| fork_schedule.in_preparation_window(fork, current_epoch))
            .unwrap_or(false);

        let mut events = vec![ForkEvent::Started {
            fork: current_fork,
            epoch: current_epoch,
        }];

        // Log scheduled fork if any
        if let Some((fork, fork_epoch)) = next_fork {
            if current_epoch < fork_epoch {
                events.push(ForkEvent::Scheduled {
                    fork,
                    fork_epoch,
                    epochs_until: fork_epoch.as_u64().saturating_sub(current_epoch.as_u64()),
                });
            }
        } else {
            // No future forks scheduled
            events.push(ForkEvent::Complete);
        }

        let state = Self {
            fork_schedule,
            current_fork,
            next_fork,
            in_preparation,
        };

        (state, events)
    }

    /// Check for state changes at the given epoch and return any events.
    pub fn check_epoch(&mut self, epoch: Epoch) -> Vec<ForkEvent> {
        let mut events = Vec::new();

        // Check for entering preparation window
        if let Some((fork, fork_epoch)) = self.next_fork {
            let now_in_preparation = self.fork_schedule.in_preparation_window(fork, epoch);
            if now_in_preparation && !self.in_preparation {
                events.push(ForkEvent::PreparationStarted {
                    fork,
                    current_epoch: epoch,
                    fork_epoch,
                    epochs_until: fork_epoch.as_u64().saturating_sub(epoch.as_u64()),
                });
                self.in_preparation = true;
            }
        }

        // Check for fork activation
        let active_fork = self.fork_schedule.active_fork(epoch);
        if active_fork != self.current_fork {
            events.push(ForkEvent::Activated {
                previous_fork: self.current_fork,
                new_fork: active_fork,
                epoch,
            });

            self.current_fork = active_fork;
            self.next_fork = self.fork_schedule.next_fork_after(epoch);
            self.in_preparation = self
                .next_fork
                .map(|(fork, _)| self.fork_schedule.in_preparation_window(fork, epoch))
                .unwrap_or(false);

            // Check if there's another fork scheduled
            if let Some((fork, fork_epoch)) = self.next_fork {
                if epoch < fork_epoch {
                    events.push(ForkEvent::Scheduled {
                        fork,
                        fork_epoch,
                        epochs_until: fork_epoch.as_u64().saturating_sub(epoch.as_u64()),
                    });
                }
            } else {
                // No more forks scheduled, we're done
                events.push(ForkEvent::Complete);
            }
        }

        events
    }

    /// Returns true if monitoring is complete (no more forks to watch).
    pub fn is_complete(&self) -> bool {
        self.next_fork.is_none()
    }

    /// Returns true if currently in the preparation window for the next fork.
    pub fn in_preparation(&self) -> bool {
        self.in_preparation
    }
}

/// Log a fork event using tracing.
fn log_event(event: &ForkEvent) {
    match event {
        ForkEvent::Started { fork, epoch } => {
            info!(fork = %fork, epoch = %epoch, "Fork monitor started");
        }
        ForkEvent::Scheduled {
            fork,
            fork_epoch,
            epochs_until,
        } => {
            info!(
                fork = %fork,
                fork_epoch = %fork_epoch,
                epochs_until = %epochs_until,
                "Fork scheduled"
            );
        }
        ForkEvent::PreparationStarted {
            fork,
            current_epoch,
            fork_epoch,
            epochs_until,
        } => {
            info!(
                fork = %fork,
                current_epoch = %current_epoch,
                fork_epoch = %fork_epoch,
                epochs_until = %epochs_until,
                "Entering fork preparation window"
            );
        }
        ForkEvent::Activated {
            previous_fork,
            new_fork,
            epoch,
        } => {
            info!(
                previous_fork = %previous_fork,
                new_fork = %new_fork,
                epoch = %epoch,
                "Fork activated"
            );
        }
        ForkEvent::Complete => {
            info!("All scheduled forks activated, fork monitor exiting");
        }
    }
}

/// Result of the monitor run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorResult {
    /// Completed successfully after all forks activated.
    Completed(Vec<ForkEvent>),
    /// Failed to determine current epoch at startup.
    NoSlotClock,
}

/// Calculate the next epoch where something interesting happens.
///
/// Returns the earlier of: preparation window start or fork activation.
fn next_interesting_epoch(schedule: &ForkSchedule, current_epoch: Epoch) -> Option<Epoch> {
    let (_, fork_epoch) = schedule.next_fork_after(current_epoch)?;
    let prep_epoch = fork_epoch.as_u64().saturating_sub(FORK_PREPARATION_EPOCHS);

    if current_epoch.as_u64() < prep_epoch {
        Some(Epoch::new(prep_epoch))
    } else {
        Some(fork_epoch)
    }
}

/// Sleep until just before the target epoch.
///
/// Wakes up 1 slot before the target epoch starts to ensure we're ready
/// when the epoch begins. This accounts for potential timing variations.
async fn sleep_until_epoch<S: SlotClock>(
    slot_clock: &S,
    target_epoch: Epoch,
    slots_per_epoch: u64,
    seconds_per_slot: u64,
) {
    let Some(current_slot) = slot_clock.now() else {
        return;
    };

    // Calculate the slot at the start of the target epoch, minus a buffer
    let target_slot = target_epoch.as_u64() * slots_per_epoch;
    let buffer_slots = 1;
    let wake_slot = target_slot.saturating_sub(buffer_slots);

    if current_slot.as_u64() >= wake_slot {
        return; // Already at or past the target
    }

    let slots_to_wait = wake_slot - current_slot.as_u64();
    let sleep_duration = Duration::from_secs(slots_to_wait * seconds_per_slot);

    tokio::time::sleep(sleep_duration).await;
}

/// Run the fork monitor, returning all events emitted.
///
/// This is the core async logic, separated from `spawn` for testability.
///
/// Instead of checking every epoch, the monitor calculates when the next
/// interesting event will occur and sleeps directly until that time.
///
/// When fork transitions occur, `ForkPhase` events are sent through the channel:
/// - `Preparing`: When entering the preparation window (time to dual-subscribe)
/// - `Activated`: When the fork activates (time to unsubscribe old topics)
pub async fn run<S: SlotClock>(
    fork_schedule: Arc<ForkSchedule>,
    slot_clock: S,
    slots_per_epoch: u64,
    seconds_per_slot: u64,
    phase_sender: ForkPhaseSender,
) -> MonitorResult {
    let mut all_events = Vec::new();

    // Get initial state
    let Some(current_epoch) = slot_clock.now().map(|s| s.epoch(slots_per_epoch)) else {
        warn!("Fork monitor: unable to determine current epoch");
        return MonitorResult::NoSlotClock;
    };

    let (mut state, initial_events) = ForkMonitorState::new(fork_schedule.clone(), current_epoch);

    // Log and collect initial events
    for event in &initial_events {
        log_event(event);
    }
    all_events.extend(initial_events);

    // Exit early if no forks to monitor
    if state.is_complete() {
        return MonitorResult::Completed(all_events);
    }

    let mut last_epoch = current_epoch;

    loop {
        // Calculate when to wake up next
        let Some(target_epoch) = next_interesting_epoch(&fork_schedule, last_epoch) else {
            break;
        };

        // Sleep until just before the target epoch
        sleep_until_epoch(&slot_clock, target_epoch, slots_per_epoch, seconds_per_slot).await;

        // Process the epoch - the state machine handles all the logic
        let Some(epoch) = slot_clock.now().map(|s| s.epoch(slots_per_epoch)) else {
            continue;
        };

        let events = state.check_epoch(epoch);
        for event in &events {
            log_event(event);

            // Send ForkPhase events to listeners
            match event {
                ForkEvent::PreparationStarted { fork, .. } => {
                    if let Some(upcoming_config) = fork_schedule.config(*fork) {
                        let _ = phase_sender
                            .send(ForkPhase::Preparing {
                                upcoming: upcoming_config.clone(),
                            })
                            .await;
                    }
                }
                ForkEvent::Activated {
                    previous_fork,
                    new_fork,
                    ..
                } => {
                    if let (Some(prev_config), Some(curr_config)) = (
                        fork_schedule.config(*previous_fork),
                        fork_schedule.config(*new_fork),
                    ) {
                        let _ = phase_sender
                            .send(ForkPhase::Activated {
                                current: curr_config.clone(),
                                previous: prev_config.clone(),
                            })
                            .await;
                    }
                }
                _ => {}
            }
        }
        all_events.extend(events);

        if state.is_complete() {
            return MonitorResult::Completed(all_events);
        }

        last_epoch = epoch;
    }

    MonitorResult::Completed(all_events)
}

/// Spawns a standalone task that monitors and logs fork transitions.
///
/// The monitor will exit automatically when all scheduled forks have activated,
/// or immediately if no forks are scheduled.
///
/// When fork transitions occur, `ForkPhase` events are sent through the channel,
/// allowing other components to react to fork changes.
pub fn spawn<S: SlotClock + 'static>(
    fork_schedule: Arc<ForkSchedule>,
    slot_clock: S,
    slots_per_epoch: u64,
    seconds_per_slot: u64,
    executor: TaskExecutor,
    phase_sender: ForkPhaseSender,
) {
    executor.spawn(
        async move {
            run(
                fork_schedule,
                slot_clock,
                slots_per_epoch,
                seconds_per_slot,
                phase_sender,
            )
            .await;
        },
        "fork_monitor",
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use slot_clock::ManualSlotClock;
    use ssv_types::domain_type::DomainType;
    use types::{ChainSpec, EthSpec, MinimalEthSpec, Slot};

    use super::*;
    use crate::FORK_PREPARATION_EPOCHS;

    // Epoch constants for state machine tests
    const BOOLE_FORK_EPOCH: u64 = 100;
    const CURRENT_EPOCH: u64 = 50;
    const PREPARATION_EPOCH: u64 = BOOLE_FORK_EPOCH - FORK_PREPARATION_EPOCHS;
    const BEFORE_PREPARATION_EPOCH: u64 = PREPARATION_EPOCH - 1;
    const AFTER_FORK_EPOCH: u64 = 150;
    const MID_EPOCH: u64 = 60;

    // Epoch constants for async activation sequence test
    const ASYNC_BOOLE_FORK_EPOCH: u64 = 10;
    const ASYNC_START_EPOCH: u64 = 8;

    // Test network name
    const TEST_NETWORK: &str = "test";

    // Test domain types
    const TEST_BASELINE_DOMAIN: DomainType = DomainType([0, 0, 0, 1]);
    const TEST_BOOLE_DOMAIN: DomainType = DomainType([0, 0, 0, 2]);

    /// Get slots per epoch from minimal spec (faster tests).
    fn slots_per_epoch() -> u64 {
        MinimalEthSpec::slots_per_epoch()
    }

    /// Get seconds per slot from minimal spec.
    fn seconds_per_slot() -> u64 {
        ChainSpec::minimal().seconds_per_slot
    }

    fn make_schedule_with_boole(boole_epoch: u64) -> Arc<ForkSchedule> {
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Alan, (Epoch::new(0), TEST_BASELINE_DOMAIN));
        configs.insert(Fork::Boole, (Epoch::new(boole_epoch), TEST_BOOLE_DOMAIN));
        Arc::new(
            ForkSchedule::from_fork_configs(configs, TEST_NETWORK).expect("valid test schedule"),
        )
    }

    fn make_schedule_no_future_forks() -> Arc<ForkSchedule> {
        // Just Alan active, no Boole scheduled
        Arc::new(ForkSchedule::new(TEST_BASELINE_DOMAIN, TEST_NETWORK))
    }

    /// Create a ManualSlotClock at the given epoch.
    fn clock_at_epoch(epoch: u64) -> ManualSlotClock {
        let clock = ManualSlotClock::new(
            Slot::new(0),
            Duration::from_secs(0),
            Duration::from_secs(seconds_per_slot()),
        );
        clock.set_slot(epoch * slots_per_epoch());
        clock
    }

    /// Duration of one epoch.
    fn epoch_duration() -> Duration {
        Duration::from_secs(slots_per_epoch() * seconds_per_slot())
    }

    /// Create test phase sender (receiver is dropped since tests verify events directly).
    fn test_phase_sender() -> ForkPhaseSender {
        let (tx, _rx) = mpsc::channel(16);
        tx
    }

    // ==================== ForkMonitorState initialization tests ====================

    #[test]
    fn test_state_new_with_scheduled_fork_emits_started_and_scheduled_events() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let (state, events) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Assert
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            ForkEvent::Started {
                fork: Fork::Alan,
                epoch: Epoch::new(CURRENT_EPOCH)
            }
        );
        assert_eq!(
            events[1],
            ForkEvent::Scheduled {
                fork: Fork::Boole,
                fork_epoch: Epoch::new(BOOLE_FORK_EPOCH),
                epochs_until: BOOLE_FORK_EPOCH - CURRENT_EPOCH
            }
        );
        assert!(!state.is_complete());
    }

    #[test]
    fn test_state_new_without_scheduled_fork_emits_started_and_complete() {
        // Arrange
        let schedule = make_schedule_no_future_forks();

        // Act
        let (state, events) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Assert
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            ForkEvent::Started {
                fork: Fork::Alan,
                epoch: Epoch::new(CURRENT_EPOCH)
            }
        );
        assert_eq!(events[1], ForkEvent::Complete);
        assert!(state.is_complete());
    }

    #[test]
    fn test_state_new_in_preparation_window_reports_scheduled_fork() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let (state, events) = ForkMonitorState::new(schedule, Epoch::new(PREPARATION_EPOCH));

        // Assert: Should report started and scheduled (we're in prep window)
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            ForkEvent::Started {
                fork: Fork::Alan,
                epoch: Epoch::new(PREPARATION_EPOCH)
            }
        );
        assert_eq!(
            events[1],
            ForkEvent::Scheduled {
                fork: Fork::Boole,
                fork_epoch: Epoch::new(BOOLE_FORK_EPOCH),
                epochs_until: FORK_PREPARATION_EPOCHS
            }
        );
        assert!(!state.is_complete());
    }

    #[test]
    fn test_state_new_after_fork_activation_starts_with_new_fork_and_completes() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let (state, events) = ForkMonitorState::new(schedule, Epoch::new(AFTER_FORK_EPOCH));

        // Assert: Boole is already active, no more forks scheduled
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            ForkEvent::Started {
                fork: Fork::Boole,
                epoch: Epoch::new(AFTER_FORK_EPOCH)
            }
        );
        assert_eq!(events[1], ForkEvent::Complete);
        assert!(state.is_complete());
    }

    // ==================== ForkMonitorState epoch progression tests ====================

    #[test]
    fn test_check_epoch_emits_preparation_event_when_entering_window() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Act: Check epoch before preparation window
        let events_before = state.check_epoch(Epoch::new(BEFORE_PREPARATION_EPOCH));

        // Act: Enter preparation window
        let events_at_prep = state.check_epoch(Epoch::new(PREPARATION_EPOCH));

        // Act: Check same epoch again (should not re-emit)
        let events_repeat = state.check_epoch(Epoch::new(PREPARATION_EPOCH));

        // Assert
        assert!(
            events_before.is_empty(),
            "No events before preparation window"
        );
        assert_eq!(events_at_prep.len(), 1);
        assert_eq!(
            events_at_prep[0],
            ForkEvent::PreparationStarted {
                fork: Fork::Boole,
                current_epoch: Epoch::new(PREPARATION_EPOCH),
                fork_epoch: Epoch::new(BOOLE_FORK_EPOCH),
                epochs_until: FORK_PREPARATION_EPOCHS
            }
        );
        assert!(
            events_repeat.is_empty(),
            "Should not emit preparation event twice"
        );
    }

    #[test]
    fn test_check_epoch_emits_activated_and_complete_at_fork_epoch() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Act
        let events = state.check_epoch(Epoch::new(BOOLE_FORK_EPOCH));

        // Assert
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            ForkEvent::Activated {
                previous_fork: Fork::Alan,
                new_fork: Fork::Boole,
                epoch: Epoch::new(BOOLE_FORK_EPOCH)
            }
        );
        assert_eq!(events[1], ForkEvent::Complete);
        assert!(state.is_complete());
    }

    #[test]
    fn test_check_epoch_preparation_then_activation_emits_both_events() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Act: Jump to preparation window
        let prep_events = state.check_epoch(Epoch::new(PREPARATION_EPOCH));

        // Act: Then activate
        let activation_events = state.check_epoch(Epoch::new(BOOLE_FORK_EPOCH));

        // Assert
        assert_eq!(prep_events.len(), 1);
        assert!(matches!(
            prep_events[0],
            ForkEvent::PreparationStarted { .. }
        ));

        assert_eq!(activation_events.len(), 2);
        assert!(matches!(activation_events[0], ForkEvent::Activated { .. }));
        assert_eq!(activation_events[1], ForkEvent::Complete);
    }

    #[test]
    fn test_check_epoch_returns_no_events_when_no_state_change() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Act: Same epoch, nothing changes
        let events_same = state.check_epoch(Epoch::new(CURRENT_EPOCH));

        // Act: Different epoch but still before preparation
        let events_mid = state.check_epoch(Epoch::new(MID_EPOCH));

        // Assert
        assert!(events_same.is_empty());
        assert!(events_mid.is_empty());
    }

    // ==================== Async run() tests ====================

    #[tokio::test]
    async fn test_run_exits_immediately_when_no_forks_scheduled() {
        // Arrange
        let schedule = make_schedule_no_future_forks();
        let clock = clock_at_epoch(CURRENT_EPOCH);
        let sender = test_phase_sender();

        // Act
        let result = run(
            schedule,
            clock,
            slots_per_epoch(),
            seconds_per_slot(),
            sender,
        )
        .await;

        // Assert
        match result {
            MonitorResult::Completed(events) => {
                assert_eq!(events.len(), 2);
                assert!(matches!(events[0], ForkEvent::Started { .. }));
                assert_eq!(events[1], ForkEvent::Complete);
            }
            _ => panic!("Expected Completed result"),
        }
    }

    #[tokio::test]
    async fn test_run_exits_immediately_when_fork_already_active() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_epoch(AFTER_FORK_EPOCH);
        let sender = test_phase_sender();

        // Act
        let result = run(
            schedule,
            clock,
            slots_per_epoch(),
            seconds_per_slot(),
            sender,
        )
        .await;

        // Assert
        match result {
            MonitorResult::Completed(events) => {
                assert_eq!(events.len(), 2);
                assert_eq!(
                    events[0],
                    ForkEvent::Started {
                        fork: Fork::Boole,
                        epoch: Epoch::new(AFTER_FORK_EPOCH)
                    }
                );
                assert_eq!(events[1], ForkEvent::Complete);
            }
            _ => panic!("Expected Completed result"),
        }
    }

    /// Tests that the monitor correctly processes fork activation over time.
    /// Uses tokio's time control to simulate epoch progression.
    #[tokio::test(start_paused = true)]
    async fn test_run_completes_full_fork_activation_sequence() {
        // Arrange
        let schedule = make_schedule_with_boole(ASYNC_BOOLE_FORK_EPOCH);
        let clock = clock_at_epoch(ASYNC_START_EPOCH);
        let sender = test_phase_sender();

        // Act: Spawn monitor and advance time through fork activation
        let monitor = tokio::spawn({
            let clock = clock.clone();
            async move {
                run(
                    schedule,
                    clock,
                    slots_per_epoch(),
                    seconds_per_slot(),
                    sender,
                )
                .await
            }
        });

        // Let the spawned task start and hit the first interval tick
        tokio::task::yield_now().await;

        // Advance through epochs until fork activation
        for epoch in (ASYNC_START_EPOCH + 1)..=ASYNC_BOOLE_FORK_EPOCH {
            clock.set_slot(epoch * slots_per_epoch());
            tokio::time::advance(epoch_duration()).await;
            tokio::task::yield_now().await;
        }

        let result = monitor.await.unwrap();

        // Assert
        match result {
            MonitorResult::Completed(events) => {
                assert!(
                    events.len() >= 3,
                    "Expected at least Started, Scheduled, and Complete events"
                );
                assert!(matches!(events[0], ForkEvent::Started { .. }));
                assert!(matches!(events[1], ForkEvent::Scheduled { .. }));
                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e, ForkEvent::Activated { .. })),
                    "Expected Activated event in sequence"
                );
                assert_eq!(events.last(), Some(&ForkEvent::Complete));
            }
            _ => panic!("Expected Completed result"),
        }
    }
}
