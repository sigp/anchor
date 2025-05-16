use std::collections::HashMap;

use dashmap::DashMap;
use ssv_types::ValidatorIndex;
use types::{PublicKeyBytes, Slot};

/// Represents an exit request scheduled for processing
#[derive(Debug, Clone)]
pub struct ExitDuty {
    pub validator_pubkey: PublicKeyBytes,
    pub validator_index: ValidatorIndex,
    pub target_slot: Slot,
}

/// Tracks voluntary exit duties for validators across slots.
#[derive(Debug)]
pub struct VoluntaryExitTracker {
    /// Maps slots to the exits that should be processed in that slot
    /// Only our own validators' exits
    scheduled_exits: DashMap<Slot, Vec<ExitDuty>>,

    /// Maps slots to a map of validator public keys and duty counts
    /// Used for duty limiting (will be used later)
    all_duties_by_slot: DashMap<Slot, HashMap<PublicKeyBytes, u64>>,
}

impl VoluntaryExitTracker {
    pub fn new() -> Self {
        Self {
            scheduled_exits: DashMap::new(),
            all_duties_by_slot: DashMap::new(),
        }
    }

    /// Add an exit duty for a validator
    /// Returns true if the exit was scheduled for processing (our validator)
    pub fn add_duty_for_slot(
        &self,
        slot: Slot,
        pubkey: PublicKeyBytes,
        validator_index: ValidatorIndex,
        is_own_validator: bool,
    ) -> bool {
        // Track this exit for duty limiting purposes

        let mut slot_map = self.all_duties_by_slot.entry(slot).or_default();
        *slot_map.entry(pubkey).or_insert(0) += 1;

        // Only schedule our own validators' exits for processing
        if is_own_validator {
            let exit_duty = ExitDuty {
                validator_pubkey: pubkey,
                validator_index,
                target_slot: slot,
            };

            self.scheduled_exits
                .entry(slot)
                .or_default()
                .push(exit_duty);
            return true;
        }
        false
    }

    /// Get all exits that should be processed at or before the given slot
    /// Returns the exits without removing them from the tracker
    pub fn get_ready_exits(&self, current_slot: Slot) -> Vec<ExitDuty> {
        let mut ready_exits = Vec::new();

        // Collect all exits at or before the current slot
        for entry in self.scheduled_exits.iter() {
            let slot = *entry.key();
            if slot <= current_slot {
                for exit in entry.value() {
                    ready_exits.push(exit.clone());
                }
            }
        }
        ready_exits
    }

    /// Remove a specific exit that has been successfully processed
    pub fn remove_processed_exit(&self, exit_duty: &ExitDuty) {
        let ExitDuty {
            validator_index,
            target_slot,
            ..
        } = exit_duty;

        if let Some(mut exits) = self.scheduled_exits.get_mut(target_slot) {
            exits.retain(|exit| exit.validator_index != *validator_index);

            // If the list is now empty, remove the slot entry entirely
            if exits.is_empty() {
                drop(exits); // Must drop the reference before removing
                self.scheduled_exits.remove(target_slot);
            }
        }
    }

    /// Get the duty count for a specific validator at a slot (for duty limiting)
    pub fn get_duty_count(&self, slot: Slot, pubkey: &PublicKeyBytes) -> u64 {
        self.all_duties_by_slot
            .get(&slot)
            .and_then(|validator_map| validator_map.get(pubkey).copied())
            .unwrap_or(0)
    }

    /// Prune old slots
    pub fn prune(&self, current_slot: Slot, lookback: u64) {
        let threshold = current_slot.saturating_sub(lookback);

        self.scheduled_exits.retain(|&slot, _| slot >= threshold);
        self.all_duties_by_slot.retain(|&slot, _| slot >= threshold);
    }
}

impl Default for VoluntaryExitTracker {
    fn default() -> Self {
        Self::new()
    }
}
