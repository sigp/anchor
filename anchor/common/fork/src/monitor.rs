//! Fork transition monitoring.
//!
//! This module provides a standalone task that monitors and logs fork transitions,
//! giving operators visibility into:
//! - Current active fork at startup
//! - Entering the preparation window before a fork
//! - Fork activation when it occurs

use std::{sync::Arc, time::Duration};

use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::time::interval;
use tracing::{info, warn};

use crate::{Fork, ForkSchedule};

/// Spawns a standalone task that monitors and logs fork transitions.
pub fn spawn<S: SlotClock + 'static>(
    fork_schedule: Arc<ForkSchedule>,
    slot_clock: S,
    slots_per_epoch: u64,
    seconds_per_slot: u64,
    executor: TaskExecutor,
) {
    executor.spawn(
        async move {
            // Get initial state
            let Some(current_epoch) = slot_clock.now().map(|s| s.epoch(slots_per_epoch)) else {
                warn!("Fork monitor: unable to determine current epoch");
                return;
            };

            let mut current_fork = fork_schedule.active_fork(current_epoch);
            let mut in_preparation =
                fork_schedule.in_preparation_window(Fork::Boole, current_epoch);

            // Log startup state
            info!(
                fork = %current_fork,
                epoch = %current_epoch,
                "Fork monitor started"
            );

            if let Some(boole_epoch) = fork_schedule.fork_epoch(Fork::Boole)
                && current_epoch < boole_epoch
            {
                let epochs_until_fork = boole_epoch.as_u64().saturating_sub(current_epoch.as_u64());
                info!(
                    boole_epoch = %boole_epoch,
                    epochs_until_fork = %epochs_until_fork,
                    "Boole fork scheduled"
                );
            }

            // Check once per epoch for state changes
            let epoch_duration = Duration::from_secs(slots_per_epoch * seconds_per_slot);
            let mut check_interval = interval(epoch_duration);

            loop {
                check_interval.tick().await;

                let Some(epoch) = slot_clock.now().map(|s| s.epoch(slots_per_epoch)) else {
                    continue;
                };

                // Check for entering preparation window
                let now_in_preparation = fork_schedule.in_preparation_window(Fork::Boole, epoch);
                if now_in_preparation && !in_preparation {
                    if let Some(boole_epoch) = fork_schedule.fork_epoch(Fork::Boole) {
                        let epochs_until_fork = boole_epoch.as_u64().saturating_sub(epoch.as_u64());
                        info!(
                            fork = "boole",
                            current_epoch = %epoch,
                            fork_epoch = %boole_epoch,
                            epochs_until_fork = %epochs_until_fork,
                            "Entering fork preparation window"
                        );
                    }
                    in_preparation = true;
                }

                // Check for fork activation
                let active_fork = fork_schedule.active_fork(epoch);
                if active_fork != current_fork {
                    info!(
                        previous_fork = %current_fork,
                        new_fork = %active_fork,
                        epoch = %epoch,
                        "Fork activated"
                    );
                    current_fork = active_fork;
                    in_preparation = false; // Reset preparation state after fork
                }
            }
        },
        "fork_monitor",
    );
}
