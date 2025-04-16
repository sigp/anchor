use std::time::{Duration, SystemTime};

use slot_clock::SlotClock;
use types::{Epoch, Slot};

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

    /// Estimates the time at the given slot
    pub fn estimated_time_at_slot(&self, slot: Slot) -> SystemTime {
        let duration = self.slot_clock.start_of(slot).unwrap_or_default();
        SystemTime::UNIX_EPOCH + duration
    }

    /// Estimates the epoch at the given slot
    pub fn estimated_epoch_at_slot(&self, slot: Slot) -> Epoch {
        Epoch::new(slot.as_u64() / self.slots_per_epoch)
    }

    /// Returns the start time of the given slot
    pub fn get_slot_start_time(&self, slot: Slot) -> SystemTime {
        self.estimated_time_at_slot(slot)
    }

    /// Checks if the given slot is the first slot of its epoch
    pub fn is_first_slot_of_epoch(&self, slot: Slot) -> bool {
        slot.as_u64() % self.slots_per_epoch == 0
    }

    /// Returns the number of epochs per sync committee period
    pub fn epochs_per_sync_committee_period(&self) -> u64 {
        self.epochs_per_sync_committee_period
    }

    /// Estimates the sync committee period at the given epoch
    pub fn estimated_sync_committee_period_at_epoch(&self, epoch: Epoch) -> u64 {
        epoch.as_u64() / self.epochs_per_sync_committee_period
    }
}
