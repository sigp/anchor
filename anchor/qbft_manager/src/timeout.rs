use std::time::Duration;

use tokio::time::Instant;

const QUICK_TIMEOUT_THRESHOLD: u64 = 8; // Round 8
const QUICK_TIMEOUT: u64 = 2; // 2 Seconds
const SLOW_TIMEOUT: u64 = 120; // 2 Minutes

pub fn calculate_round_timeout(round: u64, start_time: &Instant) -> Option<Instant> {
    let additional_timeout = if round <= QUICK_TIMEOUT_THRESHOLD {
        // If we are below the quick timeout threshold the additional timeout is round *
        // QUICK_TIMEOUT
        Duration::from_secs(round.checked_mul(QUICK_TIMEOUT)?)
    } else {
        // For higher rounds, use a combination of quick and slow timeouts

        // The quick portion is the timeout threshold * QUICK_TIMEOUT
        let quick_portion = Duration::from_secs(QUICK_TIMEOUT_THRESHOLD * QUICK_TIMEOUT);

        // The slow portion is (round - threshold) * SLOW_TIMEOUT
        let slow_portion = Duration::from_secs(
            (round.checked_sub(QUICK_TIMEOUT_THRESHOLD))?.checked_mul(SLOW_TIMEOUT)?,
        );

        quick_portion.checked_add(slow_portion)?
    };

    start_time.checked_add(additional_timeout)
}
