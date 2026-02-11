//! Fork lifecycle state management.
//!
//! Provides [`ForkLifecycle`] and [`SharedForkLifecycle`] for tracking the current
//! fork transition state across all components. The [`ForkMonitor`](crate::monitor)
//! is the sole writer; all other components read via [`SharedForkLifecycle`].

use std::sync::Arc;

use parking_lot::RwLock;
use ssv_types::domain_type::DomainType;

use crate::Fork;

/// Fork lifecycle state. Updated only by ForkMonitor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForkLifecycle {
    /// Operating on a single fork. No transition in progress.
    ///
    /// Used in two scenarios:
    /// - Pre-fork: only one fork exists (e.g., Alan at genesis).
    /// - Post-grace-period: the fork transition is complete and only
    ///   the current fork's context is relevant.
    Normal {
        current: Fork,
        domain_type: DomainType,
    },

    /// Preparing for an upcoming fork. Dual-subscribing to new topics.
    ///
    /// Both forks' contexts are relevant — peers subscribed to either
    /// the current or upcoming fork are useful.
    WarmUp {
        current: Fork,
        upcoming: Fork,
        domain_type: DomainType,
    },

    /// Fork activated but grace period still active. Keeping old subscriptions
    /// to catch late messages from the previous fork.
    ///
    /// Both forks' contexts are relevant — peers subscribed to either
    /// the current or previous fork are still useful.
    GracePeriod {
        current: Fork,
        previous: Fork,
        domain_type: DomainType,
    },
}

impl ForkLifecycle {
    /// Returns the current active fork.
    pub fn current_fork(&self) -> Fork {
        match self {
            Self::Normal { current, .. }
            | Self::WarmUp { current, .. }
            | Self::GracePeriod { current, .. } => *current,
        }
    }

    /// Returns the domain type for the current fork.
    pub fn domain_type(&self) -> DomainType {
        match self {
            Self::Normal { domain_type, .. }
            | Self::WarmUp { domain_type, .. }
            | Self::GracePeriod { domain_type, .. } => *domain_type,
        }
    }
}

/// Shared fork lifecycle state, readable by all components.
///
/// Updated only by [`ForkMonitor`](crate::monitor). Uses [`parking_lot::RwLock`]
/// for interior mutability. Writes are brief and rare (only on fork transitions),
/// so contention is negligible.
#[derive(Clone, Debug)]
pub struct SharedForkLifecycle(Arc<RwLock<ForkLifecycle>>);

impl SharedForkLifecycle {
    /// Create a new shared lifecycle with the given initial state.
    pub fn new(lifecycle: ForkLifecycle) -> Self {
        Self(Arc::new(RwLock::new(lifecycle)))
    }

    /// Read the full lifecycle state.
    pub fn get(&self) -> ForkLifecycle {
        self.0.read().clone()
    }

    /// Update the lifecycle state. Called only by ForkMonitor.
    pub fn set(&self, lifecycle: ForkLifecycle) {
        *self.0.write() = lifecycle;
    }

    /// Convenience: get current domain type without cloning full enum.
    pub fn domain_type(&self) -> DomainType {
        self.0.read().domain_type()
    }

    /// Convenience: get current fork without cloning full enum.
    pub fn current_fork(&self) -> Fork {
        self.0.read().current_fork()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALAN_DOMAIN: DomainType = DomainType([0, 0, 0, 1]);
    const BOOLE_DOMAIN: DomainType = DomainType([0, 0, 0, 2]);

    #[test]
    fn shared_fork_lifecycle_get_set_roundtrip() {
        let shared = SharedForkLifecycle::new(ForkLifecycle::Normal {
            current: Fork::Alan,
            domain_type: ALAN_DOMAIN,
        });

        assert_eq!(
            shared.get(),
            ForkLifecycle::Normal {
                current: Fork::Alan,
                domain_type: ALAN_DOMAIN,
            }
        );

        shared.set(ForkLifecycle::GracePeriod {
            current: Fork::Boole,
            previous: Fork::Alan,
            domain_type: BOOLE_DOMAIN,
        });

        assert_eq!(
            shared.get(),
            ForkLifecycle::GracePeriod {
                current: Fork::Boole,
                previous: Fork::Alan,
                domain_type: BOOLE_DOMAIN,
            }
        );
    }

    #[test]
    fn current_fork_returns_correct_fork_for_each_variant() {
        let normal = ForkLifecycle::Normal {
            current: Fork::Alan,
            domain_type: ALAN_DOMAIN,
        };
        assert_eq!(normal.current_fork(), Fork::Alan);

        let warmup = ForkLifecycle::WarmUp {
            current: Fork::Alan,
            upcoming: Fork::Boole,
            domain_type: ALAN_DOMAIN,
        };
        assert_eq!(warmup.current_fork(), Fork::Alan);

        let grace = ForkLifecycle::GracePeriod {
            current: Fork::Boole,
            previous: Fork::Alan,
            domain_type: BOOLE_DOMAIN,
        };
        assert_eq!(grace.current_fork(), Fork::Boole);
    }

    #[test]
    fn domain_type_returns_correct_type_for_each_variant() {
        let normal = ForkLifecycle::Normal {
            current: Fork::Alan,
            domain_type: ALAN_DOMAIN,
        };
        assert_eq!(normal.domain_type(), ALAN_DOMAIN);

        let warmup = ForkLifecycle::WarmUp {
            current: Fork::Alan,
            upcoming: Fork::Boole,
            domain_type: ALAN_DOMAIN,
        };
        assert_eq!(warmup.domain_type(), ALAN_DOMAIN);

        let grace = ForkLifecycle::GracePeriod {
            current: Fork::Boole,
            previous: Fork::Alan,
            domain_type: BOOLE_DOMAIN,
        };
        assert_eq!(grace.domain_type(), BOOLE_DOMAIN);
    }

    #[test]
    fn shared_convenience_methods_match_full_get() {
        let shared = SharedForkLifecycle::new(ForkLifecycle::WarmUp {
            current: Fork::Alan,
            upcoming: Fork::Boole,
            domain_type: ALAN_DOMAIN,
        });

        assert_eq!(shared.current_fork(), Fork::Alan);
        assert_eq!(shared.domain_type(), ALAN_DOMAIN);
    }
}
