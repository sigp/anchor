//! Fork lifecycle state management.
//!
//! Provides [`ForkLifecycle`] for tracking the current fork transition state
//! across all components via a `tokio::sync::watch` channel. The
//! [`ForkMonitor`](crate::monitor) is the sole writer (sender); all other
//! components hold receivers.

use crate::ForkConfig;

/// Fork lifecycle state. Updated only by ForkMonitor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForkLifecycle {
    /// Operating on a single fork. No transition in progress.
    ///
    /// Used in two scenarios:
    /// - Pre-fork: only one fork exists (e.g., Alan at genesis).
    /// - Post-grace-period: the fork transition is complete and only the current fork's context is
    ///   relevant.
    Normal { current: ForkConfig },

    /// Preparing for an upcoming fork. Dual-subscribing to new topics.
    ///
    /// Both forks' contexts are relevant — peers subscribed to either
    /// the current or upcoming fork are useful.
    WarmUp {
        current: ForkConfig,
        upcoming: ForkConfig,
    },

    /// Fork activated but grace period still active. Keeping old subscriptions
    /// to catch late messages from the previous fork.
    ///
    /// Both forks' contexts are relevant — peers subscribed to either
    /// the current or previous fork are still useful.
    GracePeriod {
        current: ForkConfig,
        previous: ForkConfig,
    },
}

impl ForkLifecycle {
    /// Returns the current active fork.
    pub fn current_fork_config(&self) -> &ForkConfig {
        match self {
            Self::Normal { current, .. }
            | Self::WarmUp { current, .. }
            | Self::GracePeriod { current, .. } => current,
        }
    }
}
