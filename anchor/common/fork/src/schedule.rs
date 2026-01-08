//! Fork schedule management.
//!
//! This module provides the `ForkSchedule` type for managing fork activations
//! and determining which fork is active at a given epoch.

use std::collections::BTreeMap;

use types::Epoch;

use crate::Fork;

/// Number of epochs before a fork to start preparing (dual-subscribing, etc.).
///
/// During this window, nodes prepare for the upcoming fork by subscribing to
/// new topics while still operating on the current fork's rules.
pub const FORK_PREPARATION_EPOCHS: u64 = 1;

/// Manages fork activation epochs and provides utilities for fork transitions.
///
/// The schedule maps each fork to its activation epoch. Forks without an
/// activation epoch are not scheduled (the network hasn't reached them yet).
#[derive(Debug, Clone)]
pub struct ForkSchedule {
    /// Maps forks to their activation epochs.
    activations: BTreeMap<Fork, Epoch>,
}

impl ForkSchedule {
    /// Create a new fork schedule with Alan active from epoch 0.
    ///
    /// All SSV networks have Alan active from the start.
    pub fn new() -> Self {
        let mut activations = BTreeMap::new();
        activations.insert(Fork::Alan, Epoch::new(0));
        Self { activations }
    }

    /// Set the activation epoch for a fork.
    pub fn set_fork_epoch(&mut self, fork: Fork, epoch: Epoch) {
        self.activations.insert(fork, epoch);
    }

    /// Get the activation epoch for a fork, if scheduled.
    pub fn fork_epoch(&self, fork: Fork) -> Option<Epoch> {
        self.activations.get(&fork).copied()
    }

    /// Get the currently active fork at the given epoch.
    ///
    /// Returns the latest fork that has activated by this epoch.
    pub fn active_fork(&self, epoch: Epoch) -> Fork {
        self.activations
            .iter()
            .filter(|&(_, &activation)| epoch >= activation)
            .max_by_key(|(_, activation)| activation.as_u64())
            .map(|(fork, _)| *fork)
            .unwrap_or(Fork::Alan)
    }

    /// Get the next scheduled fork after the given epoch.
    pub fn next_fork_after(&self, epoch: Epoch) -> Option<(Fork, Epoch)> {
        self.activations
            .iter()
            .filter(|&(_, &activation)| activation > epoch)
            .min_by_key(|(_, activation)| activation.as_u64())
            .map(|(fork, &activation)| (*fork, activation))
    }

    /// Get the epoch when preparation for a fork should begin.
    ///
    /// Returns `fork_epoch - FORK_PREPARATION_EPOCHS`, or `None` if the fork
    /// is not scheduled.
    pub fn preparation_start_epoch(&self, fork: Fork) -> Option<Epoch> {
        self.fork_epoch(fork)
            .map(|epoch| Epoch::new(epoch.as_u64().saturating_sub(FORK_PREPARATION_EPOCHS)))
    }

    /// Check if we are in the preparation window for a fork.
    ///
    /// The preparation window starts `FORK_PREPARATION_EPOCHS` before the fork
    /// and ends when the fork activates.
    pub fn in_preparation_window(&self, fork: Fork, epoch: Epoch) -> bool {
        if let (Some(prep_start), Some(fork_epoch)) =
            (self.preparation_start_epoch(fork), self.fork_epoch(fork))
        {
            epoch >= prep_start && epoch < fork_epoch
        } else {
            false
        }
    }
}

impl Default for ForkSchedule {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule_with_boole(epoch: u64) -> ForkSchedule {
        let mut schedule = ForkSchedule::new();
        schedule.set_fork_epoch(Fork::Boole, Epoch::new(epoch));
        schedule
    }

    #[test]
    fn test_new_schedule() {
        let schedule = ForkSchedule::new();
        // Alan is active from epoch 0
        assert_eq!(schedule.active_fork(Epoch::new(0)), Fork::Alan);
        assert_eq!(schedule.active_fork(Epoch::new(100)), Fork::Alan);
    }

    #[test]
    fn test_with_boole() {
        let schedule = schedule_with_boole(100);

        // Before Boole - Alan is active
        assert_eq!(schedule.active_fork(Epoch::new(50)), Fork::Alan);
        assert_eq!(schedule.active_fork(Epoch::new(99)), Fork::Alan);

        // At and after Boole
        assert_eq!(schedule.active_fork(Epoch::new(100)), Fork::Boole);
        assert_eq!(schedule.active_fork(Epoch::new(200)), Fork::Boole);
    }

    #[test]
    fn test_preparation_window() {
        let schedule = schedule_with_boole(100);

        // Before preparation window
        assert!(!schedule.in_preparation_window(Fork::Boole, Epoch::new(98)));

        // In preparation window (100 - 1 = 99)
        assert!(schedule.in_preparation_window(Fork::Boole, Epoch::new(99)));

        // At fork (no longer in preparation)
        assert!(!schedule.in_preparation_window(Fork::Boole, Epoch::new(100)));

        // After fork
        assert!(!schedule.in_preparation_window(Fork::Boole, Epoch::new(101)));
    }

    #[test]
    fn test_no_scheduled_boole() {
        let schedule = ForkSchedule::new();
        assert_eq!(schedule.active_fork(Epoch::new(1000)), Fork::Alan);
        assert_eq!(schedule.fork_epoch(Fork::Boole), None);
    }

    #[test]
    fn test_next_fork_after() {
        let schedule = schedule_with_boole(10);
        assert_eq!(
            schedule.next_fork_after(Epoch::new(0)),
            Some((Fork::Boole, Epoch::new(10)))
        );
        assert_eq!(
            schedule.next_fork_after(Epoch::new(9)),
            Some((Fork::Boole, Epoch::new(10)))
        );
        assert_eq!(schedule.next_fork_after(Epoch::new(10)), None);
    }
}
