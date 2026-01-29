use std::time::Duration;

use tokio::time::Instant;

use crate::TimeoutMode;

const QUICK_TIMEOUT_THRESHOLD: u64 = 8; // Round 8
const QUICK_TIMEOUT: u64 = 2; // 2 Seconds
const SLOW_TIMEOUT: u64 = 120; // 2 Minutes

/// Calculate when the current round should timeout.
///
/// For `SlotTime` mode: Cumulative timeout from slot start.
///   Round N ends at: start_time + sum of all round timeouts up to N.
///   Used for attestations, aggregations, sync committee duties.
///
/// For `Relative` mode: Single round timeout from the current round's start time.
///   The timer resets when we receive a justified proposal for a future round.
///   Round ends at: start_time + timeout for this round only.
///   Used for block proposals (matches Go-SSV behavior).
pub fn calculate_round_timeout(
    round: u64,
    start_time: &Instant,
    timeout_mode: TimeoutMode,
) -> Option<Instant> {
    let timeout = match timeout_mode {
        TimeoutMode::SlotTime => cumulative_timeout(round)?,
        TimeoutMode::Relative => single_round_timeout(round),
    };
    start_time.checked_add(timeout)
}

fn cumulative_timeout(round: u64) -> Option<Duration> {
    if round <= QUICK_TIMEOUT_THRESHOLD {
        // All rounds use quick timeout: round * QUICK_TIMEOUT
        Some(Duration::from_secs(round.checked_mul(QUICK_TIMEOUT)?))
    } else {
        let quick_portion = Duration::from_secs(QUICK_TIMEOUT_THRESHOLD * QUICK_TIMEOUT);

        let slow_portion = Duration::from_secs(
            (round.checked_sub(QUICK_TIMEOUT_THRESHOLD))?.checked_mul(SLOW_TIMEOUT)?,
        );

        quick_portion.checked_add(slow_portion)
    }
}

fn single_round_timeout(round: u64) -> Duration {
    if round <= QUICK_TIMEOUT_THRESHOLD {
        Duration::from_secs(QUICK_TIMEOUT)
    } else {
        Duration::from_secs(SLOW_TIMEOUT)
    }
}
