//! Fork transition monitoring.
//!
//! A standalone task derives the fork lifecycle from every fresh slot-clock
//! observation via [`ForkSchedule::lifecycle_at`] and publishes changes on a
//! watch channel. Because published state is always a pure function of the
//! observed slot, transitions land at exact slot boundaries, a jump across
//! several boundaries publishes only the state for the current slot, and a
//! restart in any window re-derives the same state.
//!
//! Sleeps are bounded to one slot so wall-clock corrections are noticed
//! promptly. Clock failures fail closed by requesting a client-wide shutdown,
//! as does a backwards clock that crosses a lifecycle boundary: published
//! state (ENR, handshake, scoring) cannot be rolled back. Both guards live as
//! long as the process but no longer: the highest observed slot is not
//! persisted, so a restart with a still-rolled-back clock re-derives and
//! re-advertises the earlier lifecycle without error.

use std::{sync::Arc, time::Duration};

use futures::channel::mpsc::Sender;
use slot_clock::SlotClock;
use task_executor::{ShutdownReason, TaskExecutor};
use tokio::sync::watch;
use tracing::{error, info, warn};
use types::{Epoch, Slot};

use crate::{FORK_PREPARATION_EPOCHS, Fork, ForkLifecycle, ForkSchedule, SUBSEQUENT_WINDOW_SLOTS};

/// Classification of a fresh slot observation against the highest slot seen.
///
/// Pure decision logic for the monitor's backwards-clock policy: all effects
/// (publishing, warning, shutdown) live in [`run`].
#[derive(Debug, PartialEq, Eq)]
enum ClockObservation {
    /// The clock moved forward or held steady.
    Advanced,
    /// The clock moved backwards, but the lifecycle derived for the observed
    /// slot matches the published one; tolerated with a warning.
    BackwardsWithinWindow,
    /// The clock moved backwards across a lifecycle boundary. Published state
    /// cannot be rolled back, so the node must fail closed.
    BackwardsAcrossBoundary,
}

/// Classify `now` against the highest slot observed so far.
fn classify_observation(
    schedule: &ForkSchedule,
    slots_per_epoch: u64,
    highest_observed_slot: Slot,
    now: Slot,
) -> ClockObservation {
    if now >= highest_observed_slot {
        ClockObservation::Advanced
    } else if schedule.lifecycle_at(now, slots_per_epoch)
        == schedule.lifecycle_at(highest_observed_slot, slots_per_epoch)
    {
        ClockObservation::BackwardsWithinWindow
    } else {
        ClockObservation::BackwardsAcrossBoundary
    }
}

/// Log an error when any fork's preparation window overlaps the previous fork's grace
/// period. `ForkSchedule::lifecycle_at` resolves such an overlap in favor of
/// `WarmUp`, which drops the previous fork's topics early; schedules should
/// keep forks at least `FORK_PREPARATION_EPOCHS` plus the subsequent window
/// apart.
fn error_on_overlapping_windows(schedule: &ForkSchedule, slots_per_epoch: u64) {
    for &fork in Fork::all() {
        let Some(config) = schedule.config(fork) else {
            continue;
        };
        let fork_epoch = config.epoch.as_u64();
        if fork_epoch == 0 {
            continue;
        }
        let previous = schedule.active_fork_config(Epoch::new(fork_epoch - 1));
        let previous_activation_slot = previous.epoch.as_u64() * slots_per_epoch;
        if previous_activation_slot == 0 {
            continue;
        }
        let previous_grace_end = previous_activation_slot + SUBSEQUENT_WINDOW_SLOTS;
        let preparation_start =
            fork_epoch.saturating_sub(FORK_PREPARATION_EPOCHS) * slots_per_epoch;
        if previous_grace_end > preparation_start {
            error!(
                fork = %fork,
                previous_fork = %previous.fork,
                "Fork preparation window overlaps the previous fork's grace period"
            );
        }
    }
}

/// Log the operator-facing message for a published lifecycle.
fn log_transition(lifecycle: &ForkLifecycle) {
    match lifecycle {
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
}

/// Request a client-wide shutdown. A stale fork lifecycle silently corrupts
/// networking, scoring, and ENR state, so clock failures must stop the node
/// rather than leave only the monitor task dead.
fn request_shutdown(shutdown_tx: &mut Sender<ShutdownReason>, reason: &'static str) {
    error!(reason, "Fork monitor failed; requesting client shutdown");
    if let Err(e) = shutdown_tx.try_send(ShutdownReason::Failure(reason))
        && !e.is_full()
    {
        // A full channel means a shutdown is already pending; a closed one means
        // there is no receiver left to act, which we can only surface in logs.
        error!("Failed to deliver shutdown request: channel closed");
    }
}

/// Run the fork monitor.
///
/// This is the core async logic, separated from `spawn` for testability.
///
/// Each iteration takes a fresh slot-clock observation, derives the lifecycle
/// for it, and publishes it if it changed, so a transition is never published
/// before its slot and a jump across several boundaries publishes only the
/// final state. An unreadable clock, or one that rolls back across a
/// lifecycle boundary, requests a client-wide shutdown (fail closed). The
/// loop never exits on its own; it is cancelled with the client.
async fn run<S: SlotClock>(
    schedule: Arc<ForkSchedule>,
    slots_per_epoch: u64,
    slot_clock: S,
    initial_slot: Slot,
    lifecycle_tx: watch::Sender<ForkLifecycle>,
    mut shutdown_tx: Sender<ShutdownReason>,
) {
    let mut highest_observed_slot = initial_slot;
    let mut boundary_unavailable_streak = 0u32;

    loop {
        let Some(now) = slot_clock.now() else {
            request_shutdown(&mut shutdown_tx, "Fork monitor: slot clock unreadable");
            return;
        };

        match classify_observation(&schedule, slots_per_epoch, highest_observed_slot, now) {
            ClockObservation::Advanced => highest_observed_slot = now,
            ClockObservation::BackwardsWithinWindow => {
                warn!(
                    observed_slot = %now,
                    highest_observed_slot = %highest_observed_slot,
                    "Slot clock moved backwards within the current fork lifecycle window"
                );
            }
            ClockObservation::BackwardsAcrossBoundary => {
                error!(
                    observed_slot = %now,
                    highest_observed_slot = %highest_observed_slot,
                    "Slot clock rolled back across a published fork transition"
                );
                request_shutdown(
                    &mut shutdown_tx,
                    "Fork monitor: clock rolled back across a published fork transition",
                );
                return;
            }
        }

        // Publish from the highest observed slot so published state never
        // regresses, even while a within-window backwards clock is tolerated.
        let lifecycle = schedule.lifecycle_at(highest_observed_slot, slots_per_epoch);
        let changed = lifecycle_tx.send_if_modified(|current| {
            if *current == lifecycle {
                false
            } else {
                *current = lifecycle.clone();
                true
            }
        });
        if changed {
            log_transition(&lifecycle);
        }

        // Target the boundary after `now` rather than a second clock reading.
        // `duration_to_slot` re-reads the clock, so `None` means the boundary
        // already passed: re-observe at once. Consecutive `None`s instead pace
        // at one slot, so a clock that never yields a boundary cannot spin.
        // The cap bounds a sleep computed against a wall clock stepped far back.
        let one_slot = slot_clock.slot_duration();
        let sleep_duration = match slot_clock.duration_to_slot(now + 1) {
            Some(until_boundary) => {
                boundary_unavailable_streak = 0;
                until_boundary.min(one_slot)
            }
            None => {
                boundary_unavailable_streak += 1;
                if boundary_unavailable_streak > 1 {
                    one_slot
                } else {
                    Duration::ZERO
                }
            }
        };
        tokio::time::sleep(sleep_duration).await;
    }
}

/// Spawns a standalone task that monitors and logs fork transitions.
///
/// The task derives the lifecycle from the slot clock once per slot for the
/// node's lifetime and publishes changes through the returned watch channel,
/// so all receivers see updates immediately.
pub fn spawn<S: SlotClock + 'static>(
    fork_schedule: Arc<ForkSchedule>,
    slot_clock: S,
    slots_per_epoch: u64,
    executor: TaskExecutor,
) -> Result<watch::Receiver<ForkLifecycle>, String> {
    let Some(current_slot) = slot_clock.now() else {
        return Err("Fork monitor: unable to determine current slot".to_string());
    };
    let current_epoch = current_slot.epoch(slots_per_epoch);
    let initial = fork_schedule.lifecycle_at(current_slot, slots_per_epoch);
    info!(
        fork = %initial.current_fork_config().fork,
        epoch = %current_epoch,
        "Fork monitor started"
    );
    if let Some((fork, fork_epoch)) = fork_schedule.next_fork_after(current_epoch) {
        info!(
            fork = %fork,
            fork_epoch = %fork_epoch,
            epochs_until = fork_epoch.as_u64().saturating_sub(current_epoch.as_u64()),
            "Fork scheduled"
        );
    }
    error_on_overlapping_windows(&fork_schedule, slots_per_epoch);

    let (lifecycle_tx, lifecycle_rx) = watch::channel(initial);
    let shutdown_tx = executor.shutdown_sender();
    executor.spawn(
        run(
            fork_schedule,
            slots_per_epoch,
            slot_clock,
            current_slot,
            lifecycle_tx,
            shutdown_tx,
        ),
        "fork_monitor",
    );

    Ok(lifecycle_rx)
}
#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use futures::channel::mpsc::{Receiver, channel};
    use slot_clock::ManualSlotClock;
    use ssv_types::domain_type::DomainType;
    use task_executor::test_utils::TestRuntime;

    use super::*;
    use crate::ForkConfig;

    /// Slots per epoch for these tests. `run` and `lifecycle_at` take this as a plain
    /// parameter, so a small value keeps the transition slots close together.
    const TEST_SLOTS_PER_EPOCH: u64 = 8;
    /// The Boole activation epoch used by most tests.
    const BOOLE_FORK_EPOCH: u64 = 2;
    /// First slot of the warm-up window: `FORK_PREPARATION_EPOCHS` before activation.
    const PREPARATION_START_SLOT: u64 =
        (BOOLE_FORK_EPOCH - FORK_PREPARATION_EPOCHS) * TEST_SLOTS_PER_EPOCH;
    /// First slot of the grace period: the Boole activation slot.
    const ACTIVATION_SLOT: u64 = BOOLE_FORK_EPOCH * TEST_SLOTS_PER_EPOCH;
    /// First slot after the grace period: back to `Normal`, now on Boole.
    const GRACE_END_SLOT: u64 = ACTIVATION_SLOT + SUBSEQUENT_WINDOW_SLOTS;

    /// Slot duration for the async loop tests, small enough to keep virtual time short.
    const TEST_SLOT_DURATION: Duration = Duration::from_secs(1);
    /// A slot duration that is not a whole number of seconds, used to catch truncation to
    /// second precision anywhere in the sleep calculation.
    const FRACTIONAL_SLOT_DURATION: Duration = Duration::from_millis(1_500);
    /// The time to a slot boundary as reported by a clock that stepped behind genesis: far
    /// longer than a slot, and far longer than the monitor may sleep for.
    const UNUSABLE_SLEEP: Duration = Duration::from_secs(3_600);

    // Test network name
    const TEST_NETWORK: &str = "test";

    // Test domain types
    const TEST_BASELINE_DOMAIN: DomainType = DomainType([0, 0, 0, 1]);
    const TEST_BOOLE_DOMAIN: DomainType = DomainType([0, 0, 0, 2]);

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

    /// Create a ManualSlotClock at the given slot, with genesis at the UNIX epoch so slot
    /// starts and virtual Tokio time share an origin.
    fn clock_at_slot(slot: u64) -> ManualSlotClock {
        let clock = ManualSlotClock::new(Slot::new(0), Duration::ZERO, TEST_SLOT_DURATION);
        clock.set_slot(slot);
        clock
    }

    /// A clock whose current slot is readable but whose reported time to any slot boundary is
    /// unusable.
    ///
    /// This is the shape `SystemTimeSlotClock` takes when it steps behind genesis between two
    /// reads: `now()` still answers from the first reading, while `duration_to_slot` takes its
    /// own wall-clock reading and reports the whole remaining time until genesis instead of
    /// the time to the requested boundary. The over-long report proves the `min(one_slot)` cap
    /// engages.
    #[derive(Clone)]
    struct UnusableNextSlotClock {
        inner: ManualSlotClock,
    }

    impl SlotClock for UnusableNextSlotClock {
        fn new(genesis_slot: Slot, genesis_duration: Duration, slot_duration: Duration) -> Self {
            Self {
                inner: ManualSlotClock::new(genesis_slot, genesis_duration, slot_duration),
            }
        }

        fn duration_to_next_slot(&self) -> Option<Duration> {
            Some(UNUSABLE_SLEEP)
        }

        fn now(&self) -> Option<Slot> {
            self.inner.now()
        }

        fn is_prior_to_genesis(&self) -> Option<bool> {
            self.inner.is_prior_to_genesis()
        }

        fn now_duration(&self) -> Option<Duration> {
            self.inner.now_duration()
        }

        fn slot_of(&self, now: Duration) -> Option<Slot> {
            self.inner.slot_of(now)
        }

        fn slot_duration(&self) -> Duration {
            self.inner.slot_duration()
        }

        fn duration_to_slot(&self, _slot: Slot) -> Option<Duration> {
            Some(UNUSABLE_SLEEP)
        }

        fn duration_to_next_epoch(&self, slots_per_epoch: u64) -> Option<Duration> {
            self.inner.duration_to_next_epoch(slots_per_epoch)
        }

        fn start_of(&self, slot: Slot) -> Option<Duration> {
            self.inner.start_of(slot)
        }

        fn genesis_slot(&self) -> Slot {
            self.inner.genesis_slot()
        }

        fn genesis_duration(&self) -> Duration {
            SlotClock::genesis_duration(&self.inner)
        }
    }

    /// A clock that advances one slot per `now()` read, whose first `duration_to_slot`
    /// query fails (the boundary-slipped race) and whose later queries resolve normally.
    ///
    /// This isolates the first step of the fallback ladder: a single `None` must re-observe
    /// immediately, which is provable because the next slot's transition publishes without
    /// any virtual time passing.
    #[derive(Clone)]
    struct BoundaryOnceUnavailableClock {
        start_slot: u64,
        now_reads: Arc<AtomicU64>,
        boundary_reads: Arc<AtomicU64>,
    }

    impl BoundaryOnceUnavailableClock {
        fn starting_at(start_slot: u64) -> Self {
            Self {
                start_slot,
                now_reads: Arc::new(AtomicU64::new(0)),
                boundary_reads: Arc::new(AtomicU64::new(0)),
            }
        }
    }

    impl SlotClock for BoundaryOnceUnavailableClock {
        fn new(_genesis_slot: Slot, _genesis_duration: Duration, _slot_duration: Duration) -> Self {
            unimplemented!("constructed via BoundaryOnceUnavailableClock::starting_at")
        }

        fn now(&self) -> Option<Slot> {
            let reads = self.now_reads.fetch_add(1, Ordering::SeqCst);
            Some(Slot::new(self.start_slot + reads))
        }

        fn is_prior_to_genesis(&self) -> Option<bool> {
            Some(false)
        }

        fn now_duration(&self) -> Option<Duration> {
            None
        }

        fn slot_of(&self, _now: Duration) -> Option<Slot> {
            None
        }

        fn slot_duration(&self) -> Duration {
            TEST_SLOT_DURATION
        }

        fn duration_to_slot(&self, _slot: Slot) -> Option<Duration> {
            (self.boundary_reads.fetch_add(1, Ordering::SeqCst) > 0).then_some(TEST_SLOT_DURATION)
        }

        fn duration_to_next_slot(&self) -> Option<Duration> {
            None
        }

        fn duration_to_next_epoch(&self, _slots_per_epoch: u64) -> Option<Duration> {
            None
        }

        fn start_of(&self, _slot: Slot) -> Option<Duration> {
            None
        }

        fn genesis_slot(&self) -> Slot {
            Slot::new(0)
        }

        fn genesis_duration(&self) -> Duration {
            Duration::ZERO
        }
    }

    /// A clock frozen at one readable slot whose `duration_to_slot` never resolves: `now()`
    /// keeps succeeding while every boundary query fails.
    ///
    /// This is the pathological shape the streak-based fallback exists for: only the first
    /// consecutive failure may re-observe immediately, every following one must pace at a
    /// full slot.
    #[derive(Clone)]
    struct FrozenNoBoundaryClock {
        slot: Slot,
    }

    impl SlotClock for FrozenNoBoundaryClock {
        fn new(_genesis_slot: Slot, _genesis_duration: Duration, _slot_duration: Duration) -> Self {
            unimplemented!("constructed directly from a slot")
        }

        fn now(&self) -> Option<Slot> {
            Some(self.slot)
        }

        fn is_prior_to_genesis(&self) -> Option<bool> {
            Some(false)
        }

        fn now_duration(&self) -> Option<Duration> {
            None
        }

        fn slot_of(&self, _now: Duration) -> Option<Slot> {
            None
        }

        fn slot_duration(&self) -> Duration {
            TEST_SLOT_DURATION
        }

        fn duration_to_slot(&self, _slot: Slot) -> Option<Duration> {
            None
        }

        fn duration_to_next_slot(&self) -> Option<Duration> {
            None
        }

        fn duration_to_next_epoch(&self, _slots_per_epoch: u64) -> Option<Duration> {
            None
        }

        fn start_of(&self, _slot: Slot) -> Option<Duration> {
            None
        }

        fn genesis_slot(&self) -> Slot {
            Slot::new(0)
        }

        fn genesis_duration(&self) -> Duration {
            Duration::ZERO
        }
    }

    /// A shutdown channel with the same capacity as the one `TaskExecutor` hands out.
    fn test_shutdown_channel() -> (Sender<ShutdownReason>, Receiver<ShutdownReason>) {
        channel(1)
    }

    /// Assert the monitor failed closed by requesting a client-wide shutdown.
    fn assert_shutdown_requested(shutdown_rx: &mut Receiver<ShutdownReason>) {
        match shutdown_rx.try_recv() {
            Ok(reason) => assert!(
                matches!(reason, ShutdownReason::Failure(_)),
                "expected a failure shutdown reason, got {reason:?}"
            ),
            Err(e) => panic!("expected a shutdown request, got {e:?}"),
        }
    }

    fn assert_no_shutdown_requested(shutdown_rx: &mut Receiver<ShutdownReason>) {
        if let Ok(reason) = shutdown_rx.try_recv() {
            panic!("unexpected shutdown request: {reason:?}");
        }
    }

    fn alan_config() -> ForkConfig {
        ForkConfig::new(Fork::Alan, Epoch::new(0), TEST_BASELINE_DOMAIN)
    }

    fn boole_config() -> ForkConfig {
        ForkConfig::new(Fork::Boole, Epoch::new(BOOLE_FORK_EPOCH), TEST_BOOLE_DOMAIN)
    }

    fn normal_alan_lifecycle() -> ForkLifecycle {
        ForkLifecycle::Normal {
            current: alan_config(),
        }
    }

    fn warmup_lifecycle() -> ForkLifecycle {
        ForkLifecycle::WarmUp {
            current: alan_config(),
            upcoming: boole_config(),
        }
    }

    fn grace_period_lifecycle() -> ForkLifecycle {
        ForkLifecycle::GracePeriod {
            current: boole_config(),
            previous: alan_config(),
        }
    }

    fn normal_boole_lifecycle() -> ForkLifecycle {
        ForkLifecycle::Normal {
            current: boole_config(),
        }
    }

    /// A monitor `run` task driven by a test clock, plus the handles a test needs to observe
    /// it.
    struct RunningMonitor {
        handle: tokio::task::JoinHandle<()>,
        lifecycle_rx: watch::Receiver<ForkLifecycle>,
        shutdown_rx: Receiver<ShutdownReason>,
    }

    /// Spawn `run` against the given clock, mirroring `spawn`'s initialization: the watch
    /// channel starts at the lifecycle derived for the clock's current slot. Yields once so
    /// the first iteration has run and the task is parked on its slot-boundary sleep.
    async fn start_run<S: SlotClock + 'static>(
        schedule: Arc<ForkSchedule>,
        clock: S,
    ) -> RunningMonitor {
        let initial_slot = clock.now().expect("test clock should be readable at start");
        let (lifecycle_tx, lifecycle_rx) =
            watch::channel(schedule.lifecycle_at(initial_slot, TEST_SLOTS_PER_EPOCH));
        let (shutdown_tx, shutdown_rx) = test_shutdown_channel();
        let handle = tokio::spawn(run(
            schedule,
            TEST_SLOTS_PER_EPOCH,
            clock,
            initial_slot,
            lifecycle_tx,
            shutdown_tx,
        ));
        tokio::task::yield_now().await;
        RunningMonitor {
            handle,
            lifecycle_rx,
            shutdown_rx,
        }
    }

    /// Fire the monitor's pending one-slot sleep and let the woken iteration run.
    async fn wake_after_one_slot() {
        tokio::time::advance(TEST_SLOT_DURATION).await;
        tokio::task::yield_now().await;
    }

    // ==================== `classify_observation` tests ====================

    #[test]
    fn test_classify_forward_observation_is_advanced() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let observation =
            classify_observation(&schedule, TEST_SLOTS_PER_EPOCH, Slot::new(5), Slot::new(6));

        // Assert
        assert_eq!(observation, ClockObservation::Advanced);
    }

    #[test]
    fn test_classify_equal_observation_is_advanced() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let observation =
            classify_observation(&schedule, TEST_SLOTS_PER_EPOCH, Slot::new(5), Slot::new(5));

        // Assert
        assert_eq!(observation, ClockObservation::Advanced);
    }

    #[test]
    fn test_classify_backwards_within_the_grace_window_is_tolerated() {
        // Arrange: both slots sit inside the grace period, so the derived lifecycle matches.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let observation = classify_observation(
            &schedule,
            TEST_SLOTS_PER_EPOCH,
            Slot::new(ACTIVATION_SLOT + 5),
            Slot::new(ACTIVATION_SLOT + 1),
        );

        // Assert
        assert_eq!(observation, ClockObservation::BackwardsWithinWindow);
    }

    #[test]
    fn test_classify_backwards_onto_the_grace_window_start_is_tolerated() {
        // Arrange: the observation lands exactly on the activation slot, the half-open
        // start of the grace window the highest slot is still inside.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let observation = classify_observation(
            &schedule,
            TEST_SLOTS_PER_EPOCH,
            Slot::new(ACTIVATION_SLOT + 5),
            Slot::new(ACTIVATION_SLOT),
        );

        // Assert
        assert_eq!(observation, ClockObservation::BackwardsWithinWindow);
    }

    #[test]
    fn test_classify_backwards_across_the_activation_boundary_is_fatal() {
        // Arrange: the highest slot is in the grace period, the observation in the warm-up
        // window just before activation.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let observation = classify_observation(
            &schedule,
            TEST_SLOTS_PER_EPOCH,
            Slot::new(ACTIVATION_SLOT),
            Slot::new(ACTIVATION_SLOT - 1),
        );

        // Assert
        assert_eq!(observation, ClockObservation::BackwardsAcrossBoundary);
    }

    #[test]
    fn test_classify_backwards_across_the_grace_end_boundary_is_fatal() {
        // Arrange: the highest slot has left the grace period, the observation is still in it.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let observation = classify_observation(
            &schedule,
            TEST_SLOTS_PER_EPOCH,
            Slot::new(GRACE_END_SLOT),
            Slot::new(GRACE_END_SLOT - 1),
        );

        // Assert
        assert_eq!(observation, ClockObservation::BackwardsAcrossBoundary);
    }

    #[test]
    fn test_classify_backwards_from_warmup_into_pre_warmup_normal_is_fatal() {
        // Arrange: the highest slot is at the exact start of the warm-up window, the
        // observation just before it, where the lifecycle is still the pre-fork Normal.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);

        // Act
        let observation = classify_observation(
            &schedule,
            TEST_SLOTS_PER_EPOCH,
            Slot::new(PREPARATION_START_SLOT),
            Slot::new(PREPARATION_START_SLOT - 1),
        );

        // Assert
        assert_eq!(observation, ClockObservation::BackwardsAcrossBoundary);
    }

    // ==================== `spawn` initial state tests ====================

    /// Spawn the monitor with the clock at the given slot and return the lifecycle receiver.
    fn spawn_monitor_at_slot(slot: u64) -> Result<watch::Receiver<ForkLifecycle>, String> {
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let runtime = TestRuntime::default();
        spawn(
            schedule,
            clock_at_slot(slot),
            TEST_SLOTS_PER_EPOCH,
            runtime.task_executor.clone(),
        )
    }

    #[tokio::test]
    async fn test_spawn_initial_state_is_normal_before_the_preparation_window() {
        // Arrange and act: the last slot before the warm-up window starts.
        let lifecycle_rx =
            spawn_monitor_at_slot(PREPARATION_START_SLOT - 1).expect("spawn should succeed");

        // Assert
        assert_eq!(*lifecycle_rx.borrow(), normal_alan_lifecycle());
    }

    #[tokio::test]
    async fn test_spawn_initial_state_is_warmup_inside_the_preparation_window() {
        // Arrange and act: the exact first slot of the warm-up window.
        let lifecycle_rx =
            spawn_monitor_at_slot(PREPARATION_START_SLOT).expect("spawn should succeed");

        // Assert
        assert_eq!(*lifecycle_rx.borrow(), warmup_lifecycle());
    }

    #[tokio::test]
    async fn test_spawn_initial_state_is_grace_period_inside_the_grace_window() {
        // Arrange and act: the exact activation slot, the first slot of the grace period.
        let lifecycle_rx = spawn_monitor_at_slot(ACTIVATION_SLOT).expect("spawn should succeed");

        // Assert
        assert_eq!(*lifecycle_rx.borrow(), grace_period_lifecycle());
    }

    #[tokio::test]
    async fn test_spawn_initial_state_is_normal_after_the_grace_window() {
        // Arrange and act: the exact first slot after the grace period.
        let lifecycle_rx = spawn_monitor_at_slot(GRACE_END_SLOT).expect("spawn should succeed");

        // Assert
        assert_eq!(*lifecycle_rx.borrow(), normal_boole_lifecycle());
    }

    #[tokio::test]
    async fn test_spawn_fails_when_the_clock_is_unreadable() {
        // Arrange: the clock sits before genesis, so `now()` returns `None`.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let genesis_time = Duration::from_secs(10);
        let clock = ManualSlotClock::new(Slot::new(0), genesis_time, TEST_SLOT_DURATION);
        clock.set_current_time(genesis_time - Duration::from_secs(1));
        let runtime = TestRuntime::default();

        // Act
        let result = spawn(
            schedule,
            clock,
            TEST_SLOTS_PER_EPOCH,
            runtime.task_executor.clone(),
        );

        // Assert
        assert!(
            result.is_err(),
            "spawn must refuse to start without a readable clock"
        );
    }

    // ==================== Async `run` tests ====================

    /// The loop sleeps only to the next slot boundary, so it wakes many times before a distant
    /// transition. Each wake must decide from a fresh observation rather than assume it is
    /// due, and the transition must land exactly when the clock reaches its slot.
    #[tokio::test(start_paused = true)]
    async fn test_run_publishes_the_warmup_transition_at_its_exact_slot() {
        // Arrange: start well before the warm-up window.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_slot(0);
        let mut monitor = start_run(schedule, clock.clone()).await;

        // Act: wake the Tokio timer repeatedly while the slot clock stays at slot 0.
        for _ in 0..3 {
            wake_after_one_slot().await;
        }

        // Assert: early wakes publish nothing.
        assert_eq!(
            *monitor.lifecycle_rx.borrow(),
            normal_alan_lifecycle(),
            "Nothing may be published while the slot clock is before the transition"
        );

        // Act and assert at the slot before the transition
        clock.set_slot(PREPARATION_START_SLOT - 1);
        wake_after_one_slot().await;
        assert_eq!(
            *monitor.lifecycle_rx.borrow(),
            normal_alan_lifecycle(),
            "The transition must not be published one slot early"
        );

        // Act and assert at the exact transition slot
        clock.set_slot(PREPARATION_START_SLOT);
        wake_after_one_slot().await;
        assert_eq!(*monitor.lifecycle_rx.borrow(), warmup_lifecycle());
        assert!(!monitor.handle.is_finished(), "Monitor must keep running");
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// Walk the clock across every boundary in order and observe each phase land at its exact
    /// slot: WarmUp at the preparation slot, GracePeriod at activation, Normal at grace end.
    #[tokio::test(start_paused = true)]
    async fn test_run_publishes_each_phase_at_its_boundary_as_the_clock_advances() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_slot(0);
        let mut monitor = start_run(schedule, clock.clone()).await;
        assert_eq!(*monitor.lifecycle_rx.borrow(), normal_alan_lifecycle());

        // Act and assert: preparation window opens.
        clock.set_slot(PREPARATION_START_SLOT);
        wake_after_one_slot().await;
        assert_eq!(*monitor.lifecycle_rx.borrow(), warmup_lifecycle());

        // Act and assert: fork activates.
        clock.set_slot(ACTIVATION_SLOT);
        wake_after_one_slot().await;
        assert_eq!(*monitor.lifecycle_rx.borrow(), grace_period_lifecycle());

        // Act and assert: grace period ends.
        clock.set_slot(GRACE_END_SLOT);
        wake_after_one_slot().await;
        assert_eq!(*monitor.lifecycle_rx.borrow(), normal_boole_lifecycle());

        assert!(!monitor.handle.is_finished(), "Monitor must keep running");
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// A clock jump across several boundaries must publish only the lifecycle derived from the
    /// current slot: the lifecycle is state, not an event stream, so replaying the
    /// intermediate states would hand receivers fork configurations that are already stale.
    #[tokio::test(start_paused = true)]
    async fn test_run_publishes_only_the_final_state_after_a_jump_across_all_boundaries() {
        // Arrange: start before the warm-up window.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_slot(0);
        let mut monitor = start_run(schedule, clock.clone()).await;

        // Act: jump the clock past every boundary, giving the loop a single wake to notice.
        clock.set_slot(GRACE_END_SLOT + 5);
        wake_after_one_slot().await;

        // Assert: exactly one change is pending and it is the final state. The jump was
        // observed in a single iteration, so at most one value was ever sent.
        assert!(
            monitor.lifecycle_rx.has_changed().expect("sender is alive"),
            "The jump must publish a change"
        );
        assert_eq!(
            *monitor.lifecycle_rx.borrow_and_update(),
            normal_boole_lifecycle(),
            "Only the state for the current slot may be published, not the skipped phases"
        );

        // Assert: nothing further is published afterwards.
        wake_after_one_slot().await;
        assert!(
            !monitor.lifecycle_rx.has_changed().expect("sender is alive"),
            "No further publishes may follow the jump"
        );
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// An unreadable clock freezes the lifecycle wherever it happened to be, which silently
    /// corrupts networking, scoring and ENR state, so the node must fail closed.
    #[tokio::test(start_paused = true)]
    async fn test_run_requests_shutdown_when_clock_becomes_unavailable() {
        // Arrange: a clock with genesis later than the time we set below.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let genesis_time = Duration::from_secs(10);
        let clock = ManualSlotClock::new(Slot::new(0), genesis_time, TEST_SLOT_DURATION);
        let mut monitor = start_run(schedule, clock.clone()).await;

        // Act: step the clock behind genesis, so `now()` returns `None` on the next wake.
        clock.set_current_time(genesis_time - Duration::from_secs(1));
        tokio::time::advance(TEST_SLOT_DURATION).await;
        monitor
            .handle
            .await
            .expect("fork monitor task should complete");

        // Assert
        assert_shutdown_requested(&mut monitor.shutdown_rx);
        assert_eq!(
            *monitor.lifecycle_rx.borrow(),
            normal_alan_lifecycle(),
            "The lifecycle must stay frozen at its last published state"
        );
    }

    /// A published lifecycle cannot be taken back, so a clock that rolls back across one
    /// leaves the node advertising a fork state its own clock says has not happened yet.
    #[tokio::test(start_paused = true)]
    async fn test_run_requests_shutdown_when_clock_rolls_back_across_published_transition() {
        // Arrange: start in the warm-up window, then publish the activation.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_slot(ACTIVATION_SLOT - 2);
        let mut monitor = start_run(schedule, clock.clone()).await;

        clock.set_slot(ACTIVATION_SLOT);
        wake_after_one_slot().await;
        assert_eq!(
            *monitor.lifecycle_rx.borrow(),
            grace_period_lifecycle(),
            "The transition should have been published before the rollback"
        );

        // Act: roll the clock back across the activation boundary.
        clock.set_slot(ACTIVATION_SLOT - 1);
        tokio::time::advance(TEST_SLOT_DURATION).await;
        monitor
            .handle
            .await
            .expect("fork monitor task should complete");

        // Assert
        assert_shutdown_requested(&mut monitor.shutdown_rx);
        assert_eq!(
            *monitor.lifecycle_rx.borrow(),
            grace_period_lifecycle(),
            "The published lifecycle must stay as it was; it cannot be rolled back"
        );
    }

    /// A clock correction that stays inside the current lifecycle window changes nothing a
    /// receiver can observe, so it is tolerated with a warning rather than being fatal, and
    /// the monitor keeps working afterwards.
    #[tokio::test(start_paused = true)]
    async fn test_run_tolerates_a_clock_rollback_within_the_current_window() {
        // Arrange: start inside the grace period.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_slot(ACTIVATION_SLOT + 5);
        let mut monitor = start_run(schedule, clock.clone()).await;

        // Act: fall back to an earlier slot that is still inside the grace period.
        clock.set_slot(ACTIVATION_SLOT + 1);
        wake_after_one_slot().await;

        // Assert: no publish, no shutdown, and the task keeps running.
        assert!(
            !monitor.lifecycle_rx.has_changed().expect("sender is alive"),
            "A within-window rollback must not publish anything"
        );
        assert_eq!(*monitor.lifecycle_rx.borrow(), grace_period_lifecycle());
        assert!(!monitor.handle.is_finished(), "Monitor must keep running");
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);

        // Act and assert: the monitor still publishes the next transition once the clock
        // moves forward again.
        clock.set_slot(GRACE_END_SLOT);
        wake_after_one_slot().await;
        assert_eq!(*monitor.lifecycle_rx.borrow(), normal_boole_lifecycle());
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// The sleep is capped at one slot regardless of what the clock reports, because a clock
    /// that stepped behind genesis reports the whole time until genesis as the time to the
    /// requested boundary. Without the cap the monitor would park for that whole span and
    /// miss every transition due in it.
    #[tokio::test(start_paused = true)]
    async fn test_run_caps_the_sleep_at_one_slot_when_the_clock_reports_a_longer_wait() {
        // Arrange
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let started_at = tokio::time::Instant::now();
        let clock = UnusableNextSlotClock::new(Slot::new(0), Duration::ZERO, TEST_SLOT_DURATION);
        let mut monitor = start_run(schedule, clock.clone()).await;

        // Act: one slot of Tokio time, against a clock asking for an hour.
        clock.inner.set_slot(PREPARATION_START_SLOT);
        wake_after_one_slot().await;

        // Assert
        assert_eq!(*monitor.lifecycle_rx.borrow(), warmup_lifecycle());
        assert!(
            started_at.elapsed() < UNUSABLE_SLEEP,
            "The monitor must re-observe within one slot, not after the reported wait"
        );
        assert!(!monitor.handle.is_finished(), "Monitor must keep running");
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// When a single `duration_to_slot` returns `None` (the boundary slipped past between
    /// the two clock reads), the loop falls back to `Duration::ZERO` and re-observes
    /// immediately instead of oversleeping. The proof: the clock advances one slot per read
    /// and reaches the preparation slot on the re-observation, so the WarmUp transition
    /// publishes without any virtual time passing at all.
    #[tokio::test(start_paused = true)]
    async fn test_run_rechecks_immediately_after_a_single_unavailable_boundary() {
        // Arrange: the first run-loop read lands one slot before the transition and its
        // boundary query fails; the immediate re-observation lands on the transition slot.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = BoundaryOnceUnavailableClock::starting_at(PREPARATION_START_SLOT - 2);
        let mut monitor = start_run(schedule, clock).await;

        // Act: yield without advancing time; only a zero-duration sleep lets the loop
        // re-observe here.
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }

        // Assert
        assert_eq!(
            *monitor.lifecycle_rx.borrow(),
            warmup_lifecycle(),
            "The re-observation after a single unavailable boundary must happen immediately"
        );
        assert!(!monitor.handle.is_finished(), "Monitor must keep running");
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// A pathological clock whose `now()` keeps answering while `duration_to_slot` never
    /// resolves must not hot-spin on the zero-sleep fallback: only the first consecutive
    /// failure re-observes immediately, every following one paces at one full slot. Under
    /// paused time a hot spin would never yield back to this test, so completing the bounded
    /// advances below is itself the proof of pacing.
    #[tokio::test(start_paused = true)]
    async fn test_run_paces_at_one_slot_when_the_boundary_stays_unavailable() {
        // Arrange: a readable slot before any transition, with no resolvable boundary.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = FrozenNoBoundaryClock { slot: Slot::new(5) };
        let mut monitor = start_run(schedule, clock).await;

        // Act: several slots of virtual time; every wake finds the boundary still
        // unavailable and must park for another full slot.
        for _ in 0..3 {
            wake_after_one_slot().await;
        }

        // Assert: the monitor is still pacing, published nothing new, and did not fail.
        assert!(!monitor.handle.is_finished(), "Monitor must keep running");
        assert_eq!(*monitor.lifecycle_rx.borrow(), normal_alan_lifecycle());
        assert!(
            !monitor.lifecycle_rx.has_changed().expect("sender is alive"),
            "A frozen clock must not produce lifecycle changes"
        );
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// Slot durations are not required to be whole seconds, so nothing in the sleep
    /// calculation may truncate to second precision and wake the monitor into an early
    /// publish.
    #[tokio::test(start_paused = true)]
    async fn test_run_publishes_at_exact_target_with_fractional_slot_duration() {
        // Arrange
        let one_millisecond = Duration::from_millis(1);
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = ManualSlotClock::new(Slot::new(0), Duration::ZERO, FRACTIONAL_SLOT_DURATION);
        let mut monitor = start_run(schedule, clock.clone()).await;

        // Act and assert at the slot before the transition
        clock.set_slot(PREPARATION_START_SLOT - 1);
        tokio::time::advance(FRACTIONAL_SLOT_DURATION).await;
        tokio::task::yield_now().await;
        assert_eq!(
            *monitor.lifecycle_rx.borrow(),
            normal_alan_lifecycle(),
            "Lifecycle must not publish one slot before the transition"
        );

        // Act and assert with the clock one millisecond before the transition slot starts
        let just_before_target = FRACTIONAL_SLOT_DURATION
            * u32::try_from(PREPARATION_START_SLOT).expect("small slot")
            - one_millisecond;
        clock.set_current_time(just_before_target);
        tokio::time::advance(FRACTIONAL_SLOT_DURATION).await;
        tokio::task::yield_now().await;
        assert_eq!(
            *monitor.lifecycle_rx.borrow(),
            normal_alan_lifecycle(),
            "A fractional slot duration must not be rounded down into an early publish"
        );
        assert!(!monitor.handle.is_finished(), "Monitor must keep running");

        // Act and assert at the exact transition slot
        clock.set_slot(PREPARATION_START_SLOT);
        tokio::time::advance(one_millisecond).await;
        tokio::task::yield_now().await;
        assert_eq!(*monitor.lifecycle_rx.borrow(), warmup_lifecycle());
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// The monitor guards against clock rollbacks for the node's lifetime, so it must keep
    /// running (and keep the watch sender alive) after the last scheduled fork's grace period
    /// has ended.
    #[tokio::test(start_paused = true)]
    async fn test_run_stays_alive_after_the_final_grace_period_ends() {
        // Arrange: start well past the grace end of the last scheduled fork.
        let schedule = make_schedule_with_boole(BOOLE_FORK_EPOCH);
        let clock = clock_at_slot(GRACE_END_SLOT + 10);
        let mut monitor = start_run(schedule, clock.clone()).await;
        assert_eq!(*monitor.lifecycle_rx.borrow(), normal_boole_lifecycle());

        // Act: keep the clock moving for several slots.
        for slot in (GRACE_END_SLOT + 11)..(GRACE_END_SLOT + 14) {
            clock.set_slot(slot);
            wake_after_one_slot().await;
        }

        // Assert: the task is still running and the sender is still alive (`has_changed`
        // returns `Ok`, not the closed-channel `Err`).
        assert!(
            !monitor.handle.is_finished(),
            "The monitor must not exit after the final grace period"
        );
        assert!(
            !monitor
                .lifecycle_rx
                .has_changed()
                .expect("the lifecycle sender must stay alive after the final grace period"),
            "No lifecycle change is expected after the final grace period"
        );
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }

    /// With no future forks scheduled there is nothing to publish, but the monitor still owns
    /// the rollback and clock-failure guards, so it keeps running.
    #[tokio::test(start_paused = true)]
    async fn test_run_keeps_running_when_no_forks_are_scheduled() {
        // Arrange
        let schedule = make_schedule_no_future_forks();
        let clock = clock_at_slot(0);
        let mut monitor = start_run(schedule, clock.clone()).await;

        // Act
        for slot in 1..4 {
            clock.set_slot(slot);
            wake_after_one_slot().await;
        }

        // Assert
        assert_eq!(*monitor.lifecycle_rx.borrow(), normal_alan_lifecycle());
        assert!(
            !monitor.handle.is_finished(),
            "The monitor must keep running with nothing scheduled"
        );
        assert_no_shutdown_requested(&mut monitor.shutdown_rx);
    }
}
