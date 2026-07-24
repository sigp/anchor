//! Fork transition monitoring.
//!
//! This module provides a standalone task that monitors and logs fork transitions,
//! giving operators visibility into:
//! - Current active fork at startup
//! - Entering the preparation window before a fork
//! - Fork activation when it occurs
//!
//! The monitor pre-computes all fork transition points at creation time from the
//! deterministic fork schedule. The `run()` method simply sleeps until each
//! transition slot and sends the corresponding lifecycle update.
//!
//! The monitor exits automatically when all scheduled forks have activated.

use std::{sync::Arc, time::Duration};

use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::sync::watch;
use tracing::{debug, error, info};
use types::{Epoch, Slot};

use crate::{FORK_PREPARATION_EPOCHS, Fork, ForkLifecycle, ForkSchedule, SUBSEQUENT_WINDOW_SLOTS};

/// A pre-computed fork transition at a specific slot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledTransition {
    slot: Slot,
    lifecycle: ForkLifecycle,
}

/// Pre-computed fork monitor with all transition points determined at creation.
///
/// The full transition timeline is deterministic from the fork schedule, so all
/// `(slot, ForkLifecycle)` pairs are computed up front. The `run()` method becomes
/// a simple sleep-and-send loop.
pub(crate) struct ForkMonitor {
    /// Lifecycle to send immediately
    initial: ForkLifecycle,
    /// Future transitions sorted by slot.
    transitions: Vec<ScheduledTransition>,
}

impl ForkMonitor {
    /// Create a new fork monitor by pre-computing all transition points.
    ///
    /// For each non-genesis fork (epoch > 0), three transitions are generated:
    /// - `WarmUp` at the preparation window start (`fork_epoch - 1` epochs)
    /// - `GracePeriod` at fork activation (`fork_epoch`)
    /// - `Normal` after the grace period (`fork_epoch + SUBSEQUENT_WINDOW_SLOTS` slots)
    ///
    /// All transitions are sorted by slot, then split at `current_slot`:
    /// - The last transition at or before `current_slot` becomes the initial state.
    /// - Remaining future transitions are stored for the run loop.
    ///
    /// This handles all restart scenarios uniformly — whether the node starts
    /// before any fork, during a warm-up window, during a grace period, or
    /// after all forks have activated.
    fn new(schedule: &ForkSchedule, current_slot: Slot, slots_per_epoch: u64) -> Self {
        let current_epoch = current_slot.epoch(slots_per_epoch);
        let current_fork_config = schedule.active_fork_config(current_epoch);
        info!(fork = %current_fork_config.fork, epoch = %current_epoch, "Fork monitor started");

        // Step 1: Generate all transitions for every non-genesis fork.
        //
        // Each fork produces three transitions at deterministic slots:
        //   prep_slot ──────> activation ──────> grace_end
        //   [WarmUp]          [GracePeriod]      [Normal]
        let mut all_transitions: Vec<ScheduledTransition> = Vec::new();

        for &fork in Fork::all() {
            let Some(fork_config) = schedule.config(fork) else {
                continue;
            };
            let fork_epoch = fork_config.epoch.as_u64();
            // Genesis forks (epoch 0) are active from the start — no transitions needed.
            if fork_epoch == 0 {
                continue;
            }

            // The fork active just before this one (safe: fork_epoch > 0).
            let prev_fork = schedule.active_fork_config(Epoch::new(fork_epoch - 1));

            // Compute the three transition slots for this fork.
            let prep_slot = fork_epoch.saturating_sub(FORK_PREPARATION_EPOCHS) * slots_per_epoch;
            let activation = fork_epoch * slots_per_epoch;
            let grace_end = activation + SUBSEQUENT_WINDOW_SLOTS;

            // Guard: the previous fork's grace period must not overlap this fork's warmup.
            let prev_activation = prev_fork.epoch.as_u64() * slots_per_epoch;
            let prev_grace_end = prev_activation + SUBSEQUENT_WINDOW_SLOTS;
            if prev_activation != 0 && prev_grace_end > prep_slot {
                error!("Fork preparation overlaps with previous's grace period");
            }

            // WarmUp: dual-subscribe to current + upcoming fork topics.
            all_transitions.push(ScheduledTransition {
                slot: Slot::new(prep_slot),
                lifecycle: ForkLifecycle::WarmUp {
                    current: prev_fork.clone(),
                    upcoming: fork_config.clone(),
                },
            });
            // GracePeriod: fork activates, keep old subscriptions for late messages.
            all_transitions.push(ScheduledTransition {
                slot: Slot::new(activation),
                lifecycle: ForkLifecycle::GracePeriod {
                    current: fork_config.clone(),
                    previous: prev_fork.clone(),
                },
            });
            // Normal: grace period ends, drop old fork subscriptions.
            all_transitions.push(ScheduledTransition {
                slot: Slot::new(grace_end),
                lifecycle: ForkLifecycle::Normal {
                    current: fork_config.clone(),
                },
            });
        }

        // Step 2: Sort by slot, then partition into past and future at current_slot.
        //
        // The half-open split [past: slot <= current] [future: slot > current] means:
        // - A transition exactly at current_slot is considered "already happened".
        // - The last past transition determines what state we should be in right now.
        // - If no transitions are past, we're in Normal (pre-fork or genesis-only).
        //
        // Example: Boole at epoch 100, restart at activation + 5:
        //   past:   [WarmUp@ep99, GracePeriod@ep100]  → initial = GracePeriod
        //   future: [Normal@ep100+32slots]             → run loop sends this later
        all_transitions.sort_by_key(|t| t.slot);
        let split = all_transitions.partition_point(|t| t.slot <= current_slot);

        let initial = if split > 0 {
            all_transitions[split - 1].lifecycle.clone()
        } else {
            ForkLifecycle::Normal {
                current: current_fork_config.clone(),
            }
        };

        let transitions = all_transitions.split_off(split);

        // Log upcoming fork.
        if let Some((fork, fork_epoch)) = schedule.next_fork_after(current_epoch) {
            info!(
                fork = %fork,
                fork_epoch = %fork_epoch,
                epochs_until = fork_epoch.as_u64().saturating_sub(current_epoch.as_u64()),
                "Fork scheduled"
            );
        }

        Self {
            initial,
            transitions,
        }
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
/// Pre-computes all transition points from the fork schedule, then sleeps
/// until each transition slot and sends the corresponding lifecycle update.
///
/// When fork transitions occur, [`ForkLifecycle`] updates are sent through the channel:
/// - `WarmUp`: When entering the preparation window (time to dual-subscribe)
/// - `GracePeriod`: When the fork activates (update ENR, but keep old subscriptions)
/// - `Normal`: When the grace period ends (time to unsubscribe old topics)
async fn run<S: SlotClock>(
    monitor: ForkMonitor,
    slot_clock: S,
    seconds_per_slot: u64,
    lifecycle_tx: watch::Sender<ForkLifecycle>,
) {
    for transition in monitor.transitions {
        sleep_until_slot(&slot_clock, transition.slot.as_u64(), seconds_per_slot).await;

        match &transition.lifecycle {
            ForkLifecycle::WarmUp {
                current, upcoming, ..
            } => {
                info!(
                    current_fork = %current.fork,
                    upcoming_fork = %upcoming.fork,
                    "Entering fork preparation window"
                );
            }
            ForkLifecycle::GracePeriod {
                current, previous, ..
            } => {
                info!(
                    previous_fork = %previous.fork,
                    new_fork = %current.fork,
                    "Fork activated"
                );
            }
            ForkLifecycle::Normal { current, .. } => {
                info!(
                    current_fork = %current.fork,
                    grace_window_slots = SUBSEQUENT_WINDOW_SLOTS,
                    "Fork transition grace period ended"
                );
            }
        }

        let _ = lifecycle_tx.send(transition.lifecycle);
    }
}

/// Spawns a standalone task that monitors and logs fork transitions.
///
/// The monitor will exit automatically when all scheduled forks have activated,
/// or immediately if no forks are scheduled.
///
/// When fork transitions occur, the new [`ForkLifecycle`] state is sent through
/// the watch channel so all receivers see the update immediately.
pub fn spawn<S: SlotClock + 'static>(
    fork_schedule: Arc<ForkSchedule>,
    slot_clock: S,
    slots_per_epoch: u64,
    seconds_per_slot: u64,
    executor: TaskExecutor,
) -> Result<watch::Receiver<ForkLifecycle>, String> {
    let Some(current_slot) = slot_clock.now() else {
        return Err("Fork monitor: unable to determine current slot".to_string());
    };

    let monitor = ForkMonitor::new(&fork_schedule, current_slot, slots_per_epoch);

    let (lifecycle_tx, lifecycle_rx) = watch::channel(monitor.initial.clone());

    executor.spawn(
        async move {
            run(monitor, slot_clock, seconds_per_slot, lifecycle_tx).await;

            debug!("No more forks scheduled, fork monitor exiting")
        },
        "fork_monitor",
    );

    Ok(lifecycle_rx)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use slot_clock::ManualSlotClock;
    use ssv_types::domain_type::DomainType;
    use types::{ChainSpec, EthSpec, MinimalEthSpec, Slot};

    use super::*;
    use crate::{FORK_PREPARATION_EPOCHS, ForkConfig};

    // Epoch constants for ForkMonitor tests
    const BOOLE_FORK_EPOCH: u64 = 100;
    const CURRENT_EPOCH: u64 = 50;
    const PREPARATION_EPOCH: u64 = BOOLE_FORK_EPOCH - FORK_PREPARATION_EPOCHS;
    const AFTER_FORK_EPOCH: u64 = 150;

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
        ChainSpec::minimal().get_slot_duration().as_secs()
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

    /// Create a test watch channel for lifecycle, returning the sender.
    /// The receiver is dropped since ForkMonitor tests verify fields directly.
    fn test_lifecycle_tx() -> watch::Sender<ForkLifecycle> {
        let (tx, _rx) = watch::channel(ForkLifecycle::Normal {
            current: ForkConfig::new(Fork::Alan, Epoch::new(0), TEST_BASELINE_DOMAIN),
        });
        tx
    }

    /// Convert epoch to slot for testing.
    fn epoch_to_slot(epoch: u64) -> Slot {
        Slot::new(epoch * slots_per_epoch())
    }

    // ==================== ForkMonitor initialization tests ====================

    #[test]
    fn test_new_with_scheduled_fork_has_transitions() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let monitor = ForkMonitor::new(&schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Assert: initial is Normal{Alan}, with 3 future transitions
        assert!(
            matches!(&monitor.initial, ForkLifecycle::Normal { current } if current.fork == Fork::Alan)
        );
        assert_eq!(
            monitor.transitions.len(),
            3,
            "Expected WarmUp, GracePeriod, Normal"
        );
        assert!(matches!(
            &monitor.transitions[0].lifecycle,
            ForkLifecycle::WarmUp { .. }
        ));
        assert!(matches!(
            &monitor.transitions[1].lifecycle,
            ForkLifecycle::GracePeriod { .. }
        ));
        assert!(matches!(
            &monitor.transitions[2].lifecycle,
            ForkLifecycle::Normal { .. }
        ));
    }

    #[test]
    fn test_new_without_future_forks_has_no_transitions() {
        // Arrange
        let schedule = make_schedule_no_future_forks();

        // Act
        let monitor = ForkMonitor::new(&schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Assert
        assert!(monitor.transitions.is_empty());
        assert!(
            matches!(&monitor.initial, ForkLifecycle::Normal { current } if current.fork == Fork::Alan)
        );
    }

    #[test]
    fn test_new_in_preparation_window_emits_initial_warmup() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let monitor = ForkMonitor::new(
            &schedule,
            epoch_to_slot(PREPARATION_EPOCH),
            slots_per_epoch(),
        );

        // Assert: initial is WarmUp (past prep slot), 2 future transitions
        assert!(
            matches!(
                &monitor.initial,
                ForkLifecycle::WarmUp { upcoming, .. } if upcoming.fork == Fork::Boole
            ),
            "Should emit WarmUp when starting in preparation window"
        );
        assert_eq!(
            monitor.transitions.len(),
            2,
            "Expected GracePeriod + Normal"
        );
        assert!(matches!(
            &monitor.transitions[0].lifecycle,
            ForkLifecycle::GracePeriod { .. }
        ));
        assert!(matches!(
            &monitor.transitions[1].lifecycle,
            ForkLifecycle::Normal { .. }
        ));
    }

    #[test]
    fn test_new_restart_at_exact_prep_slot_emits_warmup() {
        // Arrange: Start exactly at the preparation slot (lower boundary of warm-up window)
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let spe = slots_per_epoch();
        let prep_slot = Slot::new(PREPARATION_EPOCH * spe);

        // Act
        let monitor = ForkMonitor::new(&schedule, prep_slot, spe);

        // Assert: initial is WarmUp{Alan, Boole} (exact prep slot is "already happened")
        assert!(
            matches!(
                &monitor.initial,
                ForkLifecycle::WarmUp { current, upcoming }
                    if current.fork == Fork::Alan && upcoming.fork == Fork::Boole
            ),
            "Should emit WarmUp when restarting at exact preparation slot"
        );

        // Assert: 2 future transitions remain (GracePeriod + Normal)
        assert_eq!(
            monitor.transitions.len(),
            2,
            "Expected GracePeriod + Normal"
        );
    }

    #[test]
    fn test_new_after_all_forks_activated_no_transitions() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let monitor = ForkMonitor::new(
            &schedule,
            epoch_to_slot(AFTER_FORK_EPOCH),
            slots_per_epoch(),
        );

        // Assert: Boole already active, no transitions to process
        assert!(monitor.transitions.is_empty());
        assert!(
            matches!(&monitor.initial, ForkLifecycle::Normal { current, .. } if current.fork == Fork::Boole)
        );
    }

    #[test]
    fn test_new_restart_during_grace_period_emits_grace_period() {
        // Arrange: Start 1 slot after fork activation — inside the grace period window.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let grace_slot = Slot::new(BOOLE_FORK_EPOCH * slots_per_epoch() + 1);

        // Act
        let monitor = ForkMonitor::new(&schedule, grace_slot, slots_per_epoch());

        // Assert: Restart during grace period emits GracePeriod{Boole, Alan}
        assert!(
            matches!(
                &monitor.initial,
                ForkLifecycle::GracePeriod { current, previous }
                    if current.fork == Fork::Boole && previous.fork == Fork::Alan
            ),
            "Should emit GracePeriod when restarting inside the grace window"
        );

        // Assert: Exactly 1 scheduled transition — Normal at grace end
        assert_eq!(
            monitor.transitions.len(),
            1,
            "Expected exactly 1 transition (Normal at grace end)"
        );
        assert!(
            matches!(
                &monitor.transitions[0].lifecycle,
                ForkLifecycle::Normal { current } if current.fork == Fork::Boole
            ),
            "Scheduled transition should be Normal{{Boole}}"
        );
        assert_eq!(
            monitor.transitions[0].slot,
            Slot::new(BOOLE_FORK_EPOCH * slots_per_epoch() + SUBSEQUENT_WINDOW_SLOTS),
            "Normal transition should be scheduled at grace end slot"
        );
    }

    #[test]
    fn test_new_restart_at_exact_activation_emits_grace_period() {
        // Arrange: Start exactly at the fork activation slot (lower boundary of grace window).
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let activation_slot = Slot::new(BOOLE_FORK_EPOCH * slots_per_epoch());

        // Act
        let monitor = ForkMonitor::new(&schedule, activation_slot, slots_per_epoch());

        // Assert: Restart at exact activation emits GracePeriod{Boole, Alan}
        assert!(
            matches!(
                &monitor.initial,
                ForkLifecycle::GracePeriod { current, previous }
                    if current.fork == Fork::Boole && previous.fork == Fork::Alan
            ),
            "Should emit GracePeriod when restarting at exact activation slot"
        );
        assert_eq!(
            monitor.transitions.len(),
            1,
            "Expected exactly 1 transition (Normal at grace end)"
        );
    }

    #[test]
    fn test_new_restart_at_grace_end_emits_normal() {
        // Arrange: Start exactly at grace end slot — grace period has expired.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let grace_end_slot =
            Slot::new(BOOLE_FORK_EPOCH * slots_per_epoch() + SUBSEQUENT_WINDOW_SLOTS);

        // Act
        let monitor = ForkMonitor::new(&schedule, grace_end_slot, slots_per_epoch());

        // Assert: Grace period expired, initial is Normal{Boole}
        assert!(
            matches!(
                &monitor.initial,
                ForkLifecycle::Normal { current } if current.fork == Fork::Boole
            ),
            "Should emit Normal when starting at or after grace end slot"
        );

        // Assert: No transitions remaining
        assert!(
            monitor.transitions.is_empty(),
            "No transitions should be scheduled after grace period ends"
        );
    }

    // ==================== Transition slot verification tests ====================

    #[test]
    fn test_transitions_warmup_at_correct_slot() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let expected_slot = (BOOLE_FORK_EPOCH - FORK_PREPARATION_EPOCHS) * slots_per_epoch();

        // Act
        let monitor = ForkMonitor::new(&schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Assert
        assert_eq!(monitor.transitions[0].slot, Slot::new(expected_slot));
        assert!(matches!(
            &monitor.transitions[0].lifecycle,
            ForkLifecycle::WarmUp { current, upcoming }
                if current.fork == Fork::Alan
                    && upcoming.fork == Fork::Boole
                    && current.domain_type == TEST_BASELINE_DOMAIN
        ));
    }

    #[test]
    fn test_transitions_grace_period_at_correct_slot() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let expected_slot = BOOLE_FORK_EPOCH * slots_per_epoch();

        // Act
        let monitor = ForkMonitor::new(&schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Assert
        assert_eq!(monitor.transitions[1].slot, Slot::new(expected_slot));
        assert!(matches!(
            &monitor.transitions[1].lifecycle,
            ForkLifecycle::GracePeriod { current, previous }
                if current.fork == Fork::Boole
                    && previous.fork == Fork::Alan
                    && current.domain_type == TEST_BOOLE_DOMAIN
        ));
    }

    #[test]
    fn test_transitions_normal_at_correct_slot() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let expected_slot = BOOLE_FORK_EPOCH * slots_per_epoch() + SUBSEQUENT_WINDOW_SLOTS;

        // Act
        let monitor = ForkMonitor::new(&schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());

        // Assert
        assert_eq!(monitor.transitions[2].slot, Slot::new(expected_slot));
        assert!(matches!(
            &monitor.transitions[2].lifecycle,
            ForkLifecycle::Normal { current }
                if current.fork == Fork::Boole
                    && current.domain_type == TEST_BOOLE_DOMAIN
        ));
    }

    #[test]
    fn test_full_transition_sequence() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let spe = slots_per_epoch();
        let prep_slot = (BOOLE_FORK_EPOCH - FORK_PREPARATION_EPOCHS) * spe;
        let activation_slot = BOOLE_FORK_EPOCH * spe;
        let grace_end_slot = activation_slot + SUBSEQUENT_WINDOW_SLOTS;

        // Act
        let monitor = ForkMonitor::new(&schedule, epoch_to_slot(CURRENT_EPOCH), spe);

        // Assert: All 3 transitions in correct order with correct slots
        assert_eq!(monitor.transitions.len(), 3);

        assert_eq!(monitor.transitions[0].slot, Slot::new(prep_slot));
        assert!(matches!(
            &monitor.transitions[0].lifecycle,
            ForkLifecycle::WarmUp { .. }
        ));

        assert_eq!(monitor.transitions[1].slot, Slot::new(activation_slot));
        assert!(matches!(
            &monitor.transitions[1].lifecycle,
            ForkLifecycle::GracePeriod { .. }
        ));

        assert_eq!(monitor.transitions[2].slot, Slot::new(grace_end_slot));
        assert!(matches!(
            &monitor.transitions[2].lifecycle,
            ForkLifecycle::Normal { .. }
        ));
    }

    // ==================== Edge case tests ====================

    #[test]
    fn test_epoch_zero_forks_produce_no_transitions() {
        // Arrange: Both Alan and Boole at epoch 0 (all forks active from genesis).
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Alan, (Epoch::new(0), TEST_BASELINE_DOMAIN));
        configs.insert(Fork::Boole, (Epoch::new(0), TEST_BOOLE_DOMAIN));
        let schedule = Arc::new(
            ForkSchedule::from_fork_configs(configs, TEST_NETWORK).expect("valid schedule"),
        );

        // Act
        let monitor = ForkMonitor::new(&schedule, Slot::new(0), slots_per_epoch());

        // Assert: No transitions — both forks at epoch 0, Boole is the active fork.
        assert!(monitor.transitions.is_empty());
        assert!(
            matches!(&monitor.initial, ForkLifecycle::Normal { current, .. } if current.fork == Fork::Boole)
        );
    }

    #[test]
    fn test_early_fork_produces_all_transitions() {
        // Arrange: Boole at epoch 2 (very early fork).
        let schedule = make_schedule_with_boole(2);

        // Act
        let monitor = ForkMonitor::new(&schedule, Slot::new(0), slots_per_epoch());

        // Assert: All 3 transitions emitted.
        assert_eq!(monitor.transitions.len(), 3);
        assert!(matches!(
            &monitor.transitions[0].lifecycle,
            ForkLifecycle::WarmUp { .. }
        ));
        assert!(matches!(
            &monitor.transitions[1].lifecycle,
            ForkLifecycle::GracePeriod { .. }
        ));
        assert!(matches!(
            &monitor.transitions[2].lifecycle,
            ForkLifecycle::Normal { current, .. } if current.fork == Fork::Boole
        ));
    }

    // ==================== Async run() tests ====================

    #[tokio::test]
    async fn test_run_exits_immediately_when_no_forks_scheduled() {
        // Arrange
        let schedule = make_schedule_no_future_forks();
        let clock = clock_at_epoch(CURRENT_EPOCH);
        let monitor = ForkMonitor::new(&schedule, epoch_to_slot(CURRENT_EPOCH), slots_per_epoch());
        let lifecycle_tx = test_lifecycle_tx();

        // Act — completes immediately since there are no transitions
        run(monitor, clock, seconds_per_slot(), lifecycle_tx).await;
    }

    #[tokio::test]
    async fn test_run_exits_immediately_when_fork_already_active() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_epoch(AFTER_FORK_EPOCH);
        let monitor = ForkMonitor::new(
            &schedule,
            epoch_to_slot(AFTER_FORK_EPOCH),
            slots_per_epoch(),
        );
        let lifecycle_tx = test_lifecycle_tx();

        // Act — completes immediately since all forks already activated
        run(monitor, clock, seconds_per_slot(), lifecycle_tx).await;
    }

    /// Tests that the monitor correctly processes fork activation and grace period over time.
    /// Uses tokio's time control to simulate slot progression.
    #[tokio::test(start_paused = true)]
    async fn test_run_completes_full_fork_activation_sequence() {
        // Arrange
        let schedule = make_schedule_with_boole(ASYNC_BOOLE_FORK_EPOCH);
        let clock = clock_at_epoch(ASYNC_START_EPOCH);
        let monitor = ForkMonitor::new(
            &schedule,
            epoch_to_slot(ASYNC_START_EPOCH),
            slots_per_epoch(),
        );
        let (lifecycle_tx, mut lifecycle_rx) = watch::channel(monitor.initial.clone());

        // Act: Spawn monitor and advance time through fork activation and grace period
        let handle = tokio::spawn({
            let clock = clock.clone();
            async move {
                run(monitor, clock, seconds_per_slot(), lifecycle_tx).await;
            }
        });

        // Let the spawned task start and hit the first sleep
        tokio::task::yield_now().await;

        // Track observed lifecycle states
        let mut saw_warmup = false;
        let mut saw_grace_period = false;
        let mut saw_normal_boole = false;

        // Advance through epochs until fork activation
        for epoch in (ASYNC_START_EPOCH + 1)..=ASYNC_BOOLE_FORK_EPOCH {
            clock.set_slot(epoch * slots_per_epoch());
            tokio::time::advance(epoch_duration()).await;
            tokio::task::yield_now().await;
            check_lifecycle(
                &mut lifecycle_rx,
                &mut saw_warmup,
                &mut saw_grace_period,
                &mut saw_normal_boole,
            );
        }

        // Advance through the grace period (32 slots = 4 epochs on minimal spec)
        let grace_period_epochs = SUBSEQUENT_WINDOW_SLOTS / slots_per_epoch();
        for i in 1..=grace_period_epochs {
            let epoch = ASYNC_BOOLE_FORK_EPOCH + i;
            clock.set_slot(epoch * slots_per_epoch());
            tokio::time::advance(epoch_duration()).await;
            tokio::task::yield_now().await;
            check_lifecycle(
                &mut lifecycle_rx,
                &mut saw_warmup,
                &mut saw_grace_period,
                &mut saw_normal_boole,
            );
        }

        handle.await.unwrap();

        // Check final state
        check_lifecycle(
            &mut lifecycle_rx,
            &mut saw_warmup,
            &mut saw_grace_period,
            &mut saw_normal_boole,
        );

        assert!(saw_warmup, "Expected WarmUp lifecycle state");
        assert!(saw_grace_period, "Expected GracePeriod lifecycle state");
        assert!(saw_normal_boole, "Expected Normal(Boole) lifecycle state");
    }

    /// Helper to check the current lifecycle state against the expected progression.
    fn check_lifecycle(
        rx: &mut watch::Receiver<ForkLifecycle>,
        saw_warmup: &mut bool,
        saw_grace_period: &mut bool,
        saw_normal_boole: &mut bool,
    ) {
        let state = rx.borrow_and_update().clone();
        match &state {
            ForkLifecycle::WarmUp { .. } => *saw_warmup = true,
            ForkLifecycle::GracePeriod { .. } => *saw_grace_period = true,
            ForkLifecycle::Normal { current, .. } if current.fork == Fork::Boole => {
                *saw_normal_boole = true;
            }
            _ => {}
        }
    }
}
