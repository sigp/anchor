use std::time::{Duration, SystemTime, UNIX_EPOCH};

use slot_clock::SlotClock;
use types::{Epoch, Slot};

#[derive(thiserror::Error, Debug)]
pub enum TimeError {
    #[error("clock start-of-slot overflow for slot {0}")]
    Overflow(Slot),
}

/// Wrapper around SlotClock to provide beacon chain network functionality
#[derive(Clone)]
pub struct BeaconNetwork<S: SlotClock> {
    slot_clock: S,
    slots_per_epoch: u64,
    epochs_per_sync_committee_period: u64,
}

impl<S: SlotClock> BeaconNetwork<S> {
    /// Create a new BeaconNetwork
    pub fn new(slot_clock: S, slots_per_epoch: u64, epochs_per_sync_committee_period: u64) -> Self {
        Self {
            slot_clock,
            slots_per_epoch,
            epochs_per_sync_committee_period,
        }
    }

    /// Returns the slot clock
    pub fn slot_clock(&self) -> &S {
        &self.slot_clock
    }

    /// Returns the slot duration
    pub fn slot_duration(&self) -> Duration {
        self.slot_clock.slot_duration()
    }

    /// Returns the number of slots per epoch
    pub fn slots_per_epoch(&self) -> u64 {
        self.slots_per_epoch
    }

    /// Returns the start time of the given slot
    pub fn slot_start_time(&self, slot: Slot) -> Result<SystemTime, TimeError> {
        let dur = self
            .slot_clock
            .start_of(slot)
            .ok_or(TimeError::Overflow(slot))?;
        Ok(UNIX_EPOCH + dur)
    }

    /// Checks if the given slot is the first slot of its epoch
    pub fn is_first_slot_of_epoch(&self, slot: Slot) -> bool {
        slot.as_u64() % self.slots_per_epoch == 0
    }

    /// Estimates the sync committee period at the given epoch
    pub fn estimated_sync_committee_period_at_epoch(&self, epoch: Epoch) -> u64 {
        epoch.as_u64() / self.epochs_per_sync_committee_period
    }
}
