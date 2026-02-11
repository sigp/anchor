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
use tracing::{debug, info, warn};
use types::{Epoch, Slot};

use crate::{
    FORK_PREPARATION_EPOCHS, Fork, ForkConfig, ForkLifecycle, ForkSchedule,
    SUBSEQUENT_WINDOW_SLOTS, SharedForkLifecycle,
};

/// Fork transition events sent to Network and other components.
///
/// These events tell components when to:
/// - Subscribe to new topics (Preparing)
/// - Update ENR and topic prefix (Activated)
/// - Unsubscribe from old topics (GracePeriodEnded)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForkPhase {
    /// Entering preparation window - subscribe to new topics (dual-subscription).
    Preparing {
        /// Configuration for the upcoming fork.
        upcoming: ForkConfig,
    },
    /// Fork activated - update ENR and topic prefix, but keep old subscriptions.
    ///
    /// Per SIP-43, old topic subscriptions are maintained during the grace period
    /// to allow late messages from the previous fork to be processed.
    Activated {
        /// Configuration for the now-active fork.
        current: ForkConfig,
        /// Configuration for the previous fork.
        previous: ForkConfig,
    },
    /// Grace period ended - unsubscribe from old topics.
    ///
    /// This is sent `SUBSEQUENT_WINDOW_SLOTS` after fork activation, signaling
    /// that components should now unsubscribe from the previous fork's topics.
    GracePeriodEnded {
        /// Configuration for the current (active) fork.
        current: ForkConfig,
        /// Configuration for the previous fork (to unsubscribe from).
        previous: ForkConfig,
    },
}

/// Sender for fork phase events.
pub type ForkPhaseSender = async_broadcast::Sender<ForkPhase>;

/// Tracks fork monitor state and emits phases on state changes.
pub struct ForkMonitorState {
    fork_schedule: Arc<ForkSchedule>,
    current_fork: Fork,
    next_fork: Option<(Fork, Epoch)>,
    in_preparation: bool,
    /// Previous fork and slot when grace period ends (if in grace period).
    grace_period: Option<GracePeriodState>,
    /// Number of slots per epoch (needed for grace period calculation).
    slots_per_epoch: u64,
}

/// State for tracking the grace period after fork activation.
#[derive(Debug, Clone)]
struct GracePeriodState {
    /// The fork we transitioned from.
    previous_fork: Fork,
    /// The slot when the grace period ends.
    end_slot: u64,
}

impl ForkMonitorState {
    /// Create a new monitor state and log initial status.
    ///
    /// Returns:
    /// - the initialized state
    /// - `true` if there are forks to monitor (i.e., not complete on startup)
    /// - an optional initial phase to emit (e.g., if starting in preparation or grace window)
    pub fn new(
        fork_schedule: Arc<ForkSchedule>,
        current_slot: Slot,
        slots_per_epoch: u64,
    ) -> (Self, bool, Option<ForkPhase>) {
        let current_epoch = current_slot.epoch(slots_per_epoch);
        let current_fork = fork_schedule.active_fork(current_epoch);
        let next_fork = fork_schedule.next_fork_after(current_epoch);
        let in_preparation = next_fork
            .map(|(fork, _)| fork_schedule.in_preparation_window(fork, current_epoch))
            .unwrap_or(false);

        // Log startup info
        info!(fork = %current_fork, epoch = %current_epoch, "Fork monitor started");

        let current_fork_epoch = fork_schedule
            .fork_epoch(current_fork)
            .unwrap_or(Epoch::new(0));
        let mut grace_previous_fork: Option<Fork> = None;
        let mut grace_period = None;

        if let Some((previous_fork, _)) = fork_schedule.fork_before_epoch(current_fork_epoch) {
            let activation_slot = current_fork_epoch.as_u64() * slots_per_epoch;
            let grace_end_slot = activation_slot + SUBSEQUENT_WINDOW_SLOTS;
            let current_slot_u64 = current_slot.as_u64();

            if current_slot_u64 >= activation_slot && current_slot_u64 < grace_end_slot {
                grace_previous_fork = Some(previous_fork);
                grace_period = Some(GracePeriodState {
                    previous_fork,
                    end_slot: grace_end_slot,
                });
            }
        }

        let has_future_fork = if let Some((fork, fork_epoch)) = next_fork {
            if current_epoch < fork_epoch {
                info!(
                    fork = %fork,
                    fork_epoch = %fork_epoch,
                    epochs_until = fork_epoch.as_u64().saturating_sub(current_epoch.as_u64()),
                    "Fork scheduled"
                );
            }
            true
        } else {
            false
        };

        let has_work = has_future_fork || grace_period.is_some();
        if !has_future_fork && grace_period.is_none() {
            info!("All scheduled forks activated, fork monitor exiting");
        }

        let state = Self {
            fork_schedule,
            current_fork,
            next_fork,
            in_preparation,
            grace_period,
            slots_per_epoch,
        };

        let initial_phase = if state.in_preparation {
            state
                .next_fork
                .and_then(|(fork, _)| state.fork_schedule.config(fork))
                .map(|config| ForkPhase::Preparing {
                    upcoming: config.clone(),
                })
        } else if let Some(previous_fork) = grace_previous_fork {
            match (
                state.fork_schedule.config(previous_fork),
                state.fork_schedule.config(state.current_fork),
            ) {
                (Some(previous), Some(current)) => Some(ForkPhase::Activated {
                    current: current.clone(),
                    previous: previous.clone(),
                }),
                _ => None,
            }
        } else {
            None
        };

        (state, has_work, initial_phase)
    }

    /// Check for state changes at the given slot and return any phases to emit.
    ///
    /// The `current_slot` is used for precise grace period tracking, while
    /// fork activation and preparation are still tracked at the epoch level.
    pub fn check_slot(&mut self, current_slot: Slot) -> Vec<ForkPhase> {
        let epoch = current_slot.epoch(self.slots_per_epoch);

        [
            self.check_preparation_window(epoch),
            self.check_fork_activation(epoch),
            self.check_grace_period_ended(current_slot, epoch),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// Check if we're entering the preparation window for the next fork.
    fn check_preparation_window(&mut self, epoch: Epoch) -> Option<ForkPhase> {
        let (fork, fork_epoch) = self.next_fork?;

        if self.in_preparation {
            return None;
        }

        if !self.fork_schedule.in_preparation_window(fork, epoch) {
            return None;
        }

        info!(
            fork = %fork,
            current_epoch = %epoch,
            fork_epoch = %fork_epoch,
            epochs_until = fork_epoch.as_u64().saturating_sub(epoch.as_u64()),
            "Entering fork preparation window"
        );

        self.in_preparation = true;

        self.fork_schedule
            .config(fork)
            .map(|config| ForkPhase::Preparing {
                upcoming: config.clone(),
            })
    }

    /// Check if a fork has activated at this epoch.
    fn check_fork_activation(&mut self, epoch: Epoch) -> Option<ForkPhase> {
        let active_fork = self.fork_schedule.active_fork(epoch);

        if active_fork == self.current_fork {
            return None;
        }

        let previous_fork = self.current_fork;

        info!(
            previous_fork = %previous_fork,
            new_fork = %active_fork,
            epoch = %epoch,
            "Fork activated"
        );

        // Start tracking the grace period
        let fork_activation_slot = epoch.as_u64() * self.slots_per_epoch;
        self.grace_period = Some(GracePeriodState {
            previous_fork,
            end_slot: fork_activation_slot + SUBSEQUENT_WINDOW_SLOTS,
        });

        // Update state for next fork
        self.current_fork = active_fork;
        self.next_fork = self.fork_schedule.next_fork_after(epoch);
        self.in_preparation = self
            .next_fork
            .map(|(fork, _)| self.fork_schedule.in_preparation_window(fork, epoch))
            .unwrap_or(false);

        // Log if there's another fork scheduled
        if let Some((fork, fork_epoch)) = self.next_fork.filter(|(_, fe)| epoch < *fe) {
            info!(
                fork = %fork,
                fork_epoch = %fork_epoch,
                epochs_until = fork_epoch.as_u64().saturating_sub(epoch.as_u64()),
                "Fork scheduled"
            );
        }

        // Build the phase if we have both configs
        let prev_config = self.fork_schedule.config(previous_fork)?;
        let curr_config = self.fork_schedule.config(active_fork)?;

        Some(ForkPhase::Activated {
            current: curr_config.clone(),
            previous: prev_config.clone(),
        })
    }

    /// Check if the grace period after fork activation has ended.
    fn check_grace_period_ended(&mut self, current_slot: Slot, epoch: Epoch) -> Option<ForkPhase> {
        let grace = self.grace_period.as_ref()?;

        if current_slot.as_u64() < grace.end_slot {
            return None;
        }

        let grace = self.grace_period.take().expect("checked above");

        info!(
            previous_fork = %grace.previous_fork,
            current_fork = %self.current_fork,
            epoch = %epoch,
            grace_window_slots = SUBSEQUENT_WINDOW_SLOTS,
            "Fork transition grace period ended, unsubscribing from old topics"
        );

        // Log completion if no more forks
        if self.next_fork.is_none() {
            info!("All scheduled forks activated, fork monitor exiting");
        }

        // Build the phase if we have both configs
        let prev_config = self.fork_schedule.config(grace.previous_fork)?;
        let curr_config = self.fork_schedule.config(self.current_fork)?;

        Some(ForkPhase::GracePeriodEnded {
            current: curr_config.clone(),
            previous: prev_config.clone(),
        })
    }

    /// Returns true if monitoring is complete (no more forks to watch and grace period ended).
    pub fn is_complete(&self) -> bool {
        self.next_fork.is_none() && self.grace_period.is_none()
    }

    /// Returns true if currently in the preparation window for the next fork.
    pub fn in_preparation(&self) -> bool {
        self.in_preparation
    }

    /// Returns the slot when the current grace period ends, if in a grace period.
    pub fn grace_period_end_slot(&self) -> Option<Slot> {
        self.grace_period.as_ref().map(|g| Slot::new(g.end_slot))
    }

    /// Returns the current fork being monitored.
    pub fn current_fork(&self) -> Fork {
        self.current_fork
    }
}

/// Result of the monitor run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorResult {
    /// Completed successfully after all forks activated.
    Completed,
    /// Failed to determine current epoch at startup.
    NoSlotClock,
}

/// Calculate the next slot where something interesting happens.
///
/// Returns the earlier of:
/// - Preparation window start (epoch-based)
/// - Fork activation (epoch-based)
/// - Grace period end (slot-based, if in grace period)
fn next_interesting_slot(
    schedule: &ForkSchedule,
    current_slot: u64,
    slots_per_epoch: u64,
    grace_period_end_slot: Option<u64>,
) -> Option<u64> {
    let current_epoch = Epoch::new(current_slot / slots_per_epoch);

    // If we're in a grace period and haven't passed it yet, consider it for wakeup
    if let Some(end_slot) = grace_period_end_slot.filter(|&end| current_slot < end) {
        // If there's also a next fork, wake up at the earlier of the two
        if let Some((_, fork_epoch)) = schedule.next_fork_after(current_epoch) {
            let prep_epoch = fork_epoch.as_u64().saturating_sub(FORK_PREPARATION_EPOCHS);
            let epoch_slot = if current_epoch.as_u64() < prep_epoch {
                prep_epoch * slots_per_epoch
            } else {
                fork_epoch.as_u64() * slots_per_epoch
            };
            return Some(end_slot.min(epoch_slot));
        }
        return Some(end_slot);
    }

    // Otherwise, wake up for the next fork event
    let (_, fork_epoch) = schedule.next_fork_after(current_epoch)?;
    let prep_epoch = fork_epoch.as_u64().saturating_sub(FORK_PREPARATION_EPOCHS);

    if current_epoch.as_u64() < prep_epoch {
        Some(prep_epoch * slots_per_epoch)
    } else {
        Some(fork_epoch.as_u64() * slots_per_epoch)
    }
}

/// Sleep until just before the target slot.
///
/// Wakes up 1 slot before the target to ensure we're ready.
/// This accounts for potential timing variations.
async fn sleep_until_slot<S: SlotClock>(slot_clock: &S, target_slot: u64, seconds_per_slot: u64) {
    let Some(current_slot) = slot_clock.now() else {
        return;
    };

    // Wake up 1 slot before the target
    let buffer_slots = 1;
    let wake_slot = target_slot.saturating_sub(buffer_slots);

    if current_slot.as_u64() >= wake_slot {
        return; // Already at or past the target
    }

    let slots_to_wait = wake_slot - current_slot.as_u64();
    let sleep_duration = Duration::from_secs(slots_to_wait * seconds_per_slot);

    tokio::time::sleep(sleep_duration).await;
}

/// Run the fork monitor.
///
/// This is the core async logic, separated from `spawn` for testability.
///
/// Instead of checking every slot, the monitor calculates when the next
/// interesting event will occur and sleeps directly until that time.
///
/// When fork transitions occur, `ForkPhase` events are sent through the channel:
/// - `Preparing`: When entering the preparation window (time to dual-subscribe)
/// - `Activated`: When the fork activates (update ENR, but keep old subscriptions)
/// - `GracePeriodEnded`: When the grace period ends (time to unsubscribe old topics)
pub async fn run<S: SlotClock>(
    fork_schedule: Arc<ForkSchedule>,
    slot_clock: S,
    slots_per_epoch: u64,
    seconds_per_slot: u64,
    phase_sender: ForkPhaseSender,
    fork_lifecycle: SharedForkLifecycle,
) -> MonitorResult {
    // Get initial state
    let Some(current_slot) = slot_clock.now() else {
        return MonitorResult::NoSlotClock;
    };
    let (mut state, has_work, initial_phase) =
        ForkMonitorState::new(fork_schedule.clone(), current_slot, slots_per_epoch);

    // Exit early if no forks to monitor
    if !has_work || state.is_complete() {
        return MonitorResult::Completed;
    }

    if let Some(phase) = &initial_phase {
        update_lifecycle_for_phase(&fork_lifecycle, phase, &fork_schedule, state.current_fork());
    }

    if let Some(phase) = initial_phase {
        let _ = phase_sender.broadcast_direct(phase).await;
    }

    let mut last_slot = current_slot.as_u64();

    loop {
        // Calculate when to wake up next
        let grace_period_end = state.grace_period_end_slot().map(|s| s.as_u64());
        let Some(target_slot) =
            next_interesting_slot(&fork_schedule, last_slot, slots_per_epoch, grace_period_end)
        else {
            break;
        };

        // Sleep until just before the target slot
        sleep_until_slot(&slot_clock, target_slot, seconds_per_slot).await;

        // Process the slot - the state machine handles all the logic
        let Some(slot) = slot_clock.now() else {
            continue;
        };

        let phases = state.check_slot(slot);
        for phase in &phases {
            update_lifecycle_for_phase(
                &fork_lifecycle,
                phase,
                &fork_schedule,
                state.current_fork(),
            );
        }
        for phase in phases {
            let _ = phase_sender.broadcast_direct(phase).await;
        }

        if state.is_complete() {
            return MonitorResult::Completed;
        }

        last_slot = slot.as_u64();
    }

    MonitorResult::Completed
}

/// Update the shared fork lifecycle based on a fork phase event.
///
/// Called right before broadcasting each phase event so readers see the new state
/// at the same time or before listeners process the event.
fn update_lifecycle_for_phase(
    fork_lifecycle: &SharedForkLifecycle,
    phase: &ForkPhase,
    fork_schedule: &ForkSchedule,
    current_fork: Fork,
) {
    match phase {
        ForkPhase::Preparing { upcoming } => {
            // During preparation, current fork's domain type is still active
            let Some(domain_type) = fork_schedule.domain_type(current_fork) else {
                tracing::error!(
                    fork = %current_fork,
                    "Missing domain type for current fork during preparation"
                );
                return;
            };
            fork_lifecycle.set(ForkLifecycle::WarmUp {
                current: current_fork,
                upcoming: upcoming.fork,
                domain_type,
            });
        }
        ForkPhase::Activated { current, previous } => {
            fork_lifecycle.set(ForkLifecycle::GracePeriod {
                current: current.fork,
                previous: previous.fork,
                domain_type: current.domain_type,
            });
        }
        ForkPhase::GracePeriodEnded { current, .. } => {
            fork_lifecycle.set(ForkLifecycle::Normal {
                current: current.fork,
                domain_type: current.domain_type,
            });
        }
    }
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
    fork_lifecycle: SharedForkLifecycle,
) {
    executor.spawn(
        async move {
            let result = run(
                fork_schedule,
                slot_clock,
                slots_per_epoch,
                seconds_per_slot,
                phase_sender,
                fork_lifecycle,
            )
            .await;

            match result {
                MonitorResult::Completed => debug!("No more forks scheduled, fork monitor exiting"),
                MonitorResult::NoSlotClock => {
                    warn!("Fork monitor: unable to determine current slot")
                }
            }
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
        Arc::new(ForkSchedule::new(
            Fork::Alan,
            TEST_BASELINE_DOMAIN,
            TEST_NETWORK,
        ))
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
        let (tx, _rx) = async_broadcast::broadcast(16);
        tx
    }

    /// Create a test SharedForkLifecycle starting on Alan.
    fn test_fork_lifecycle() -> SharedForkLifecycle {
        SharedForkLifecycle::new(ForkLifecycle::Normal {
            current: Fork::Alan,
            domain_type: TEST_BASELINE_DOMAIN,
        })
    }

    /// Convert epoch to slot for testing.
    fn epoch_to_slot(epoch: u64) -> Slot {
        Slot::new(epoch * slots_per_epoch())
    }

    // ==================== ForkMonitorState initialization tests ====================

    #[test]
    fn test_state_new_with_scheduled_fork_has_work() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let (state, has_work, _) =
            ForkMonitorState::new(schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Assert
        assert!(has_work, "Should have work when fork is scheduled");
        assert!(!state.is_complete());
        assert_eq!(state.current_fork(), Fork::Alan);
    }

    #[test]
    fn test_state_new_without_scheduled_fork_has_no_work() {
        // Arrange
        let schedule = make_schedule_no_future_forks();

        // Act
        let (state, has_work, _) =
            ForkMonitorState::new(schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Assert
        assert!(!has_work, "Should not have work when no forks scheduled");
        assert!(state.is_complete());
    }

    #[test]
    fn test_state_new_in_preparation_window_has_work() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let (state, has_work, initial_phase) = ForkMonitorState::new(
            schedule,
            epoch_to_slot(PREPARATION_EPOCH),
            slots_per_epoch(),
        );

        // Assert
        assert!(has_work, "Should have work when in preparation window");
        assert!(!state.is_complete());
        assert!(
            matches!(initial_phase, Some(ForkPhase::Preparing { upcoming }) if upcoming.fork == Fork::Boole),
            "Should emit Preparing when starting in preparation window"
        );
    }

    #[test]
    fn test_state_new_after_fork_activation_has_no_work() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let (state, has_work, _) =
            ForkMonitorState::new(schedule, epoch_to_slot(AFTER_FORK_EPOCH), slots_per_epoch());

        // Assert: Boole is already active, no more forks scheduled
        assert!(!has_work, "Should not have work when all forks activated");
        assert!(state.is_complete());
        assert_eq!(state.current_fork(), Fork::Boole);
    }

    #[test]
    fn test_state_new_in_grace_period_emits_activated() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let grace_slot = Slot::new(BOOLE_FORK_EPOCH * slots_per_epoch() + 1);

        // Act
        let (state, has_work, initial_phase) =
            ForkMonitorState::new(schedule, grace_slot, slots_per_epoch());

        // Assert
        assert!(has_work, "Should keep monitoring during grace period");
        assert!(!state.is_complete());
        assert!(
            matches!(initial_phase, Some(ForkPhase::Activated { current, previous })
                if current.fork == Fork::Boole && previous.fork == Fork::Alan),
            "Should emit Activated when starting in grace period"
        );
        assert!(state.grace_period_end_slot().is_some());
    }

    // ==================== ForkMonitorState slot progression tests ====================

    #[test]
    fn test_check_slot_emits_preparing_when_entering_window() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _, _) =
            ForkMonitorState::new(schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Act: Check slot before preparation window
        let phases_before = state.check_slot(epoch_to_slot(BEFORE_PREPARATION_EPOCH));

        // Act: Enter preparation window
        let phases_at_prep = state.check_slot(epoch_to_slot(PREPARATION_EPOCH));

        // Act: Check same slot again (should not re-emit)
        let phases_repeat = state.check_slot(epoch_to_slot(PREPARATION_EPOCH));

        // Assert
        assert!(
            phases_before.is_empty(),
            "No phases before preparation window"
        );
        assert_eq!(phases_at_prep.len(), 1);
        assert!(
            matches!(&phases_at_prep[0], ForkPhase::Preparing { upcoming } if upcoming.fork == Fork::Boole)
        );
        assert!(
            phases_repeat.is_empty(),
            "Should not emit preparation phase twice"
        );
    }

    #[test]
    fn test_check_slot_emits_activated_at_fork_epoch() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _, _) =
            ForkMonitorState::new(schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Act: Check at fork activation slot
        let phases = state.check_slot(epoch_to_slot(BOOLE_FORK_EPOCH));

        // Assert: Should emit Activated
        assert_eq!(phases.len(), 1);
        assert!(matches!(
            &phases[0],
            ForkPhase::Activated { current, previous }
            if current.fork == Fork::Boole && previous.fork == Fork::Alan
        ));
        // State is NOT complete because grace period hasn't ended
        assert!(!state.is_complete());
        assert!(state.grace_period_end_slot().is_some());
    }

    #[test]
    fn test_check_slot_emits_grace_period_ended_after_window() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _, _) =
            ForkMonitorState::new(schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Act: Activate the fork
        let _ = state.check_slot(epoch_to_slot(BOOLE_FORK_EPOCH));

        // Get the grace period end slot
        let grace_end = state
            .grace_period_end_slot()
            .expect("should be in grace period");

        // Act: Check at grace period end
        let phases = state.check_slot(grace_end);

        // Assert: Should emit GracePeriodEnded
        assert_eq!(phases.len(), 1);
        assert!(matches!(
            &phases[0],
            ForkPhase::GracePeriodEnded { current, previous }
            if current.fork == Fork::Boole && previous.fork == Fork::Alan
        ));
        assert!(state.is_complete());
    }

    #[test]
    fn test_check_slot_full_sequence() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _, _) =
            ForkMonitorState::new(schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Act: Jump to preparation window
        let prep_phases = state.check_slot(epoch_to_slot(PREPARATION_EPOCH));

        // Act: Then activate
        let activation_phases = state.check_slot(epoch_to_slot(BOOLE_FORK_EPOCH));

        // Act: Then end grace period
        let grace_end = state
            .grace_period_end_slot()
            .expect("should be in grace period");
        let grace_phases = state.check_slot(grace_end);

        // Assert
        assert_eq!(prep_phases.len(), 1);
        assert!(matches!(prep_phases[0], ForkPhase::Preparing { .. }));

        assert_eq!(activation_phases.len(), 1);
        assert!(matches!(activation_phases[0], ForkPhase::Activated { .. }));

        assert_eq!(grace_phases.len(), 1);
        assert!(matches!(
            grace_phases[0],
            ForkPhase::GracePeriodEnded { .. }
        ));

        assert!(state.is_complete());
    }

    #[test]
    fn test_check_slot_returns_no_phases_when_no_state_change() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let (mut state, _, _) =
            ForkMonitorState::new(schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Act: Same slot, nothing changes
        let phases_same = state.check_slot(epoch_to_slot(CURRENT_EPOCH));

        // Act: Different slot but still before preparation
        let phases_mid = state.check_slot(epoch_to_slot(MID_EPOCH));

        // Assert
        assert!(phases_same.is_empty());
        assert!(phases_mid.is_empty());
    }

    // ==================== Async run() tests ====================

    #[tokio::test]
    async fn test_run_exits_immediately_when_no_forks_scheduled() {
        // Arrange
        let schedule = make_schedule_no_future_forks();
        let clock = clock_at_epoch(CURRENT_EPOCH);
        let sender = test_phase_sender();
        let lifecycle = test_fork_lifecycle();

        // Act
        let result = run(
            schedule,
            clock,
            slots_per_epoch(),
            seconds_per_slot(),
            sender,
            lifecycle,
        )
        .await;

        // Assert
        assert_eq!(result, MonitorResult::Completed);
    }

    #[tokio::test]
    async fn test_run_exits_immediately_when_fork_already_active() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_epoch(AFTER_FORK_EPOCH);
        let sender = test_phase_sender();
        let lifecycle = test_fork_lifecycle();

        // Act
        let result = run(
            schedule,
            clock,
            slots_per_epoch(),
            seconds_per_slot(),
            sender,
            lifecycle,
        )
        .await;

        // Assert
        assert_eq!(result, MonitorResult::Completed);
    }

    /// Tests that the monitor correctly processes fork activation and grace period over time.
    /// Uses tokio's time control to simulate slot progression.
    #[tokio::test(start_paused = true)]
    async fn test_run_completes_full_fork_activation_sequence() {
        // Arrange
        let schedule = make_schedule_with_boole(ASYNC_BOOLE_FORK_EPOCH);
        let clock = clock_at_epoch(ASYNC_START_EPOCH);
        let (sender, mut receiver) = async_broadcast::broadcast(16);
        let lifecycle = test_fork_lifecycle();

        // Act: Spawn monitor and advance time through fork activation and grace period
        let monitor = tokio::spawn({
            let clock = clock.clone();
            async move {
                run(
                    schedule,
                    clock,
                    slots_per_epoch(),
                    seconds_per_slot(),
                    sender,
                    lifecycle,
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

        // Advance through the grace period (32 slots = 4 epochs on minimal spec)
        let grace_period_epochs = SUBSEQUENT_WINDOW_SLOTS / slots_per_epoch();
        for i in 1..=grace_period_epochs {
            let epoch = ASYNC_BOOLE_FORK_EPOCH + i;
            clock.set_slot(epoch * slots_per_epoch());
            tokio::time::advance(epoch_duration()).await;
            tokio::task::yield_now().await;
        }

        let result = monitor.await.unwrap();

        // Assert
        assert_eq!(result, MonitorResult::Completed);

        // Collect received phases
        let mut phases = Vec::new();
        while let Ok(phase) = receiver.try_recv() {
            phases.push(phase);
        }

        // Should have received at least Preparing, Activated, and GracePeriodEnded
        assert!(
            phases
                .iter()
                .any(|p| matches!(p, ForkPhase::Preparing { .. })),
            "Expected Preparing phase"
        );
        assert!(
            phases
                .iter()
                .any(|p| matches!(p, ForkPhase::Activated { .. })),
            "Expected Activated phase"
        );
        assert!(
            phases
                .iter()
                .any(|p| matches!(p, ForkPhase::GracePeriodEnded { .. })),
            "Expected GracePeriodEnded phase"
        );
    }
}
