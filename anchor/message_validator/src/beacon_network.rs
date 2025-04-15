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

    /// Estimates the current slot
    pub fn estimated_current_slot(&self) -> Slot {
        self.slot_clock.now().unwrap_or_default()
    }

    /// Estimates the slot at the given time
    pub fn estimated_slot_at_time(&self, time: SystemTime) -> Slot {
        let since_unix = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();

        self.slot_clock.slot_of(since_unix).unwrap_or_default()
    }

    /// Estimates the time at the given slot
    pub fn estimated_time_at_slot(&self, slot: Slot) -> SystemTime {
        let duration = self.slot_clock.start_of(slot).unwrap_or_default();
        SystemTime::UNIX_EPOCH + duration
    }

    /// Estimates the current epoch
    pub fn estimated_current_epoch(&self) -> Epoch {
        self.estimated_epoch_at_slot(self.estimated_current_slot())
    }

    /// Estimates the epoch at the given slot
    pub fn estimated_epoch_at_slot(&self, slot: Slot) -> Epoch {
        Epoch::new(slot.as_u64() / self.slots_per_epoch)
    }

    /// Returns the first slot at the given epoch
    pub fn first_slot_at_epoch(&self, epoch: u64) -> Slot {
        Slot::new(epoch * self.slots_per_epoch)
    }

    /// Returns the start time of the given epoch
    pub fn epoch_start_time(&self, epoch: u64) -> SystemTime {
        self.estimated_time_at_slot(self.first_slot_at_epoch(epoch))
    }

    /// Returns the start time of the given slot
    pub fn get_slot_start_time(&self, slot: Slot) -> SystemTime {
        self.estimated_time_at_slot(slot)
    }

    /// Returns the end time of the given slot
    pub fn get_slot_end_time(&self, slot: Slot) -> SystemTime {
        self.estimated_time_at_slot(slot + 1)
    }

    /// Checks if the given slot is the first slot of its epoch
    pub fn is_first_slot_of_epoch(&self, slot: Slot) -> bool {
        slot.as_u64() % self.slots_per_epoch == 0
    }

    /// Returns the first slot of the given epoch
    pub fn get_epoch_first_slot(&self, epoch: u64) -> Slot {
        self.first_slot_at_epoch(epoch)
    }

    /// Returns the number of epochs per sync committee period
    pub fn epochs_per_sync_committee_period(&self) -> u64 {
        self.epochs_per_sync_committee_period
    }

    /// Estimates the sync committee period at the given epoch
    pub fn estimated_sync_committee_period_at_epoch(&self, epoch: Epoch) -> u64 {
        epoch.as_u64() / self.epochs_per_sync_committee_period
    }

    /// Returns the first epoch of the given sync committee period
    pub fn first_epoch_of_sync_period(&self, period: u64) -> u64 {
        period * self.epochs_per_sync_committee_period
    }

    /// Returns the last slot of the given sync committee period
    pub fn last_slot_of_sync_period(&self, period: u64) -> Slot {
        let last_epoch = self.first_epoch_of_sync_period(period + 1) - 1;
        // If we are in the sync committee that ends at slot x we do not generate a message
        // during slot x-1 as it will never be included, hence -2.
        self.get_epoch_first_slot(last_epoch + 1) - 2
    }
}
