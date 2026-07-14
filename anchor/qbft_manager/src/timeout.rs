use std::time::Duration;

use tokio::time::Instant;

use crate::TimeoutMode;

const QUICK_TIMEOUT_THRESHOLD: u64 = 8; // Round 8
const QUICK_TIMEOUT: u64 = 2; // 2 Seconds
const SLOW_TIMEOUT: u64 = 120; // 2 Minutes

/// Calculate when the current round should timeout.
///
/// For `SlotTime` mode: Cumulative timeout from a fixed origin instant.
///   Round N ends at: `round_deadline_origin` + sum of all round timeouts up to N.
///   The instance may initialize before or after the origin without shifting the
///   deadlines; a deadline already in the past fires immediately, cascading round
///   changes until the rounds catch up.
///   Used for attestations, aggregations, sync committee duties.
///
/// For `Relative` mode: Single round timeout from the current round's start time.
///   The timer resets when the round advances.
///   Round ends at: current_round_start_time + timeout for this round only.
///   Used for block proposals (matches Go-SSV behavior).
pub fn calculate_round_timeout(round: u64, timeout_mode: TimeoutMode) -> Option<Instant> {
    match timeout_mode {
        TimeoutMode::SlotTime {
            round_deadline_origin,
        } => round_deadline_origin.checked_add(cumulative_timeout(round)?),
        TimeoutMode::Relative {
            current_round_start_time,
        } => current_round_start_time.checked_add(single_round_timeout(round)),
    }
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
