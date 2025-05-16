use std::{ops::Add, sync::LazyLock, time::Duration};

use slot_clock::SlotClock;
use ssv_types::{Round, msgid::Role};
use types::Slot;

use crate::InstanceHeight;

pub static QUICK_TIMEOUT_THRESHOLD: LazyLock<Round> = LazyLock::new(|| Round::from(8));

const QUICK_TIMEOUT: u64 = 2; // 2 Seconds
const SLOW_TIMEOUT: u64 = 120; // 2 Minutes

pub fn calculate_round_timeout<T: SlotClock + 'static>(
    role: Option<Role>,
    height: &InstanceHeight,
    round: &Round,
    slot_clock: &T,
) -> Duration {
    let slot = Slot::new(**height as u64);
    let time_to_slot_start = slot_clock.duration_to_slot(slot).unwrap_or_default();

    let role = if let Some(role) = role {
        role
    } else {
        return time_to_slot_start;
    };

    let slot_duration = slot_clock.slot_duration();

    // Set base duration based on role
    let base_duration = match role {
        Role::Committee => {
            // thid of the slot time
            slot_duration / 3
        }
        Role::Aggregator | Role::SyncCommittee => {
            // two-thirds of the slot time
            slot_duration * 2 / 3
        }
        _ => {
            if round.get() <= QUICK_TIMEOUT_THRESHOLD.get() {
                Duration::from_secs(QUICK_TIMEOUT)
            } else {
                Duration::from_secs(SLOW_TIMEOUT)
            }
        }
    };

    // Additional timeout based on round
    let additional_timeout = if round.get() <= QUICK_TIMEOUT_THRESHOLD.get() {
        Duration::from_secs(round.get() as u64 * QUICK_TIMEOUT)
    } else {
        // For higher rounds, use a combination of quick and slow timeouts
        let quick_portion = Duration::from_secs(QUICK_TIMEOUT_THRESHOLD.get() as u64 * 2);
        let slow_portion = Duration::from_secs(
            (round.get() as u64 - QUICK_TIMEOUT_THRESHOLD.get() as u64) * SLOW_TIMEOUT,
        );
        quick_portion + slow_portion
    };

    let total_timeout = base_duration.add(additional_timeout);

    time_to_slot_start + total_timeout
}
