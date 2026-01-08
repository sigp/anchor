//! Fork transition monitoring.
//!
//! This module provides a standalone task that monitors and logs fork transitions,
//! giving operators visibility into:
//! - Current active fork at startup
//! - Entering the preparation window before a fork
//! - Fork activation when it occurs
//!
//! The monitor exits automatically when all scheduled forks have activated.

use std::{sync::Arc, time::Duration};

use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::time::interval;
use tracing::{info, warn};
use types::Epoch;

use crate::{Fork, ForkSchedule};

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

/// Run the fork monitor, returning all events emitted.
///
/// This is the core async logic, separated from `spawn` for testability.
pub async fn run<S: SlotClock>(
    fork_schedule: Arc<ForkSchedule>,
    slot_clock: S,
    slots_per_epoch: u64,
    seconds_per_slot: u64,
) -> MonitorResult {
    let mut all_events = Vec::new();

    // Get initial state
    let Some(current_epoch) = slot_clock.now().map(|s| s.epoch(slots_per_epoch)) else {
        warn!("Fork monitor: unable to determine current epoch");
        return MonitorResult::NoSlotClock;
    };

    let (mut state, initial_events) = ForkMonitorState::new(fork_schedule, current_epoch);

    // Log and collect initial events
    for event in &initial_events {
        log_event(event);
    }
    all_events.extend(initial_events);

    // Exit early if no forks to monitor
    if state.is_complete() {
        return MonitorResult::Completed(all_events);
    }

    // Check once per epoch for state changes
    let epoch_duration = Duration::from_secs(slots_per_epoch * seconds_per_slot);
    let mut check_interval = interval(epoch_duration);

    loop {
        check_interval.tick().await;

        let Some(epoch) = slot_clock.now().map(|s| s.epoch(slots_per_epoch)) else {
            continue;
        };

        let events = state.check_epoch(epoch);
        for event in &events {
            log_event(event);
        }
        all_events.extend(events);

        // Exit if monitoring is complete
        if state.is_complete() {
            return MonitorResult::Completed(all_events);
        }
    }
}

/// Spawns a standalone task that monitors and logs fork transitions.
///
/// The monitor will exit automatically when all scheduled forks have activated,
/// or immediately if no forks are scheduled.
pub fn spawn<S: SlotClock + 'static>(
    fork_schedule: Arc<ForkSchedule>,
    slot_clock: S,
    slots_per_epoch: u64,
    seconds_per_slot: u64,
    executor: TaskExecutor,
) {
    executor.spawn(
        async move {
            run(fork_schedule, slot_clock, slots_per_epoch, seconds_per_slot).await;
        },
        "fork_monitor",
    );
}

#[cfg(test)]
mod tests {
    use slot_clock::ManualSlotClock;
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

    /// Get slots per epoch from minimal spec (faster tests).
    fn slots_per_epoch() -> u64 {
        MinimalEthSpec::slots_per_epoch()
    }

    /// Get seconds per slot from minimal spec.
    fn seconds_per_slot() -> u64 {
        ChainSpec::minimal().seconds_per_slot
    }

    fn make_schedule_with_boole(boole_epoch: u64) -> Arc<ForkSchedule> {
        let mut schedule = ForkSchedule::new();
        schedule.set_fork_epoch(Fork::Boole, Epoch::new(boole_epoch));
        Arc::new(schedule)
    }

    fn make_schedule_no_future_forks() -> Arc<ForkSchedule> {
        // Just Genesis/Alan active, no Boole scheduled
        Arc::new(ForkSchedule::new())
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

    // ==================== ForkMonitorState unit tests ====================

    #[test]
    fn test_startup_with_scheduled_fork() {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (state, events) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

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
    fn test_startup_no_scheduled_fork() {
        let schedule = make_schedule_no_future_forks();
        let (state, events) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

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
    fn test_preparation_window_entry() {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Before preparation window
        let events = state.check_epoch(Epoch::new(BEFORE_PREPARATION_EPOCH));
        assert!(events.is_empty());

        // Enter preparation window
        let events = state.check_epoch(Epoch::new(PREPARATION_EPOCH));
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ForkEvent::PreparationStarted {
                fork: Fork::Boole,
                current_epoch: Epoch::new(PREPARATION_EPOCH),
                fork_epoch: Epoch::new(BOOLE_FORK_EPOCH),
                epochs_until: FORK_PREPARATION_EPOCHS
            }
        );

        // Should not emit again
        let events = state.check_epoch(Epoch::new(PREPARATION_EPOCH));
        assert!(events.is_empty());
    }

    #[test]
    fn test_fork_activation() {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Activate fork
        let events = state.check_epoch(Epoch::new(BOOLE_FORK_EPOCH));
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
    fn test_fork_activation_includes_preparation() {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Jump directly to preparation window
        let events = state.check_epoch(Epoch::new(PREPARATION_EPOCH));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ForkEvent::PreparationStarted { .. }));

        // Then activate
        let events = state.check_epoch(Epoch::new(BOOLE_FORK_EPOCH));
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ForkEvent::Activated { .. }));
        assert_eq!(events[1], ForkEvent::Complete);
    }

    #[test]
    fn test_no_events_when_nothing_changes() {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _) = ForkMonitorState::new(schedule, Epoch::new(CURRENT_EPOCH));

        // Same epoch, nothing changes
        let events = state.check_epoch(Epoch::new(CURRENT_EPOCH));
        assert!(events.is_empty());

        // Different epoch but still before preparation
        let events = state.check_epoch(Epoch::new(MID_EPOCH));
        assert!(events.is_empty());
    }

    #[test]
    fn test_startup_already_in_preparation() {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (state, events) = ForkMonitorState::new(schedule, Epoch::new(PREPARATION_EPOCH));

        // Should report started and scheduled (we're in prep window)
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
    fn test_startup_after_fork_already_active() {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (state, events) = ForkMonitorState::new(schedule, Epoch::new(AFTER_FORK_EPOCH));

        // Boole is already active, no more forks scheduled
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

    // ==================== Async run() tests ====================

    #[tokio::test]
    async fn test_run_immediate_exit_no_scheduled_forks() {
        let schedule = make_schedule_no_future_forks();
        let clock = clock_at_epoch(CURRENT_EPOCH);

        let result = run(schedule, clock, slots_per_epoch(), seconds_per_slot()).await;

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
    async fn test_run_immediate_exit_fork_already_active() {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_epoch(AFTER_FORK_EPOCH);

        let result = run(schedule, clock, slots_per_epoch(), seconds_per_slot()).await;

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

    #[tokio::test(start_paused = true)]
    async fn test_run_fork_activation_sequence() {
        let schedule = make_schedule_with_boole(ASYNC_BOOLE_FORK_EPOCH);
        let clock = clock_at_epoch(ASYNC_START_EPOCH);

        let monitor = tokio::spawn({
            let clock = clock.clone();
            async move { run(schedule, clock, slots_per_epoch(), seconds_per_slot()).await }
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

        match result {
            MonitorResult::Completed(events) => {
                // Must have Started, Scheduled at minimum, and Complete at end
                assert!(events.len() >= 3);
                assert!(matches!(events[0], ForkEvent::Started { .. }));
                assert!(matches!(events[1], ForkEvent::Scheduled { .. }));
                // Must have Activated event
                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e, ForkEvent::Activated { .. }))
                );
                assert_eq!(events.last(), Some(&ForkEvent::Complete));
            }
            _ => panic!("Expected Completed result"),
        }
    }
}
