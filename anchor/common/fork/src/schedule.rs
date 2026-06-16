//! Fork schedule management.
//!
//! This module provides the `ForkSchedule` type for managing fork activations
//! and determining which fork is active at a given epoch.

use std::collections::BTreeMap;

use ssv_types::domain_type::DomainType;
use types::Epoch;

use crate::Fork;

/// Number of epochs before a fork to start preparing (dual-subscribing, etc.).
///
/// During this window, nodes prepare for the upcoming fork by subscribing to
/// new topics while still operating on the current fork's rules.
///
/// # Rationale for 1 Epoch
///
/// This is set to 1 epoch (~6.4 minutes on mainnet) which provides sufficient
/// time for gossipsub mesh formation while minimizing the dual-subscription
/// overhead. The gossipsub heartbeat interval (1s) allows approximately 384
/// heartbeats for peer discovery and mesh grafting during preparation.
///
/// A shorter window reduces the resource overhead of maintaining dual subscriptions
/// (memory for extra mesh connections, bandwidth for duplicate topic advertisements).
/// The 1-epoch window was chosen as a conservative balance between preparation time
/// and operational overhead.
///
/// # Failure Modes Considered
///
/// - **Node restarts during preparation**: Nodes that restart during the preparation window will
///   immediately re-subscribe to both old and new topics on startup (if still in the preparation
///   window based on current epoch).
/// - **Network partitions**: Nodes that miss the preparation window entirely will subscribe to new
///   topics at fork activation, which may result in brief message delays while gossipsub meshes
///   form.
///
/// This value can be increased if network simulations or mainnet experience
/// indicate that more preparation time is needed.
pub const FORK_PREPARATION_EPOCHS: u64 = 1;

/// Number of slots after fork activation to remain subscribed to old topics.
///
/// Per SIP-43, this grace period allows late messages from the previous fork
/// to be processed during the transition. Messages for pre-fork slots are
/// still valid during this window.
///
/// # Rationale for 32 Slots
///
/// This value is chosen to accommodate the message TTL for Committee and
/// Aggregator roles, which can be up to ~34 slots. The 32-slot window covers
/// the vast majority of legitimate late messages while bounding the
/// dual-subscription overhead.
///
/// After this window expires:
/// - Old topic subscriptions are removed
/// - Messages for pre-fork slots are dropped
pub const SUBSEQUENT_WINDOW_SLOTS: u64 = 32;

/// Complete configuration for a fork, including computed values.
///
/// This is the single source of truth for all fork-related values.
/// The `topic_prefix` is computed from the fork and network name at config creation time.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ForkConfig {
    /// Which fork this configuration is for.
    pub fork: Fork,
    /// The epoch at which this fork activates.
    pub epoch: Epoch,
    /// The domain type used for message signing in this fork.
    pub domain_type: DomainType,
}

impl ForkConfig {
    /// Create a new fork configuration with computed topic prefix.
    pub fn new(fork: Fork, epoch: Epoch, domain_type: DomainType) -> Self {
        Self {
            fork,
            epoch,
            domain_type,
        }
    }
}

/// Manages fork activation epochs and provides utilities for fork transitions.
///
/// The schedule maps each fork to its configuration (epoch, domain type, and topic prefix).
/// Forks without a configuration are not scheduled.
#[derive(Debug, Clone)]
pub struct ForkSchedule {
    /// Maps forks to their configuration.
    configs: BTreeMap<Fork, ForkConfig>,
    /// Network name used for topic prefix computation.
    network_name: String,
}

impl ForkSchedule {
    /// Create a new fork schedule with the given fork active from epoch 0.
    pub fn new(fork: Fork, baseline_domain_type: DomainType, network_name: &str) -> Self {
        let mut configs = BTreeMap::new();
        for fork in Fork::all().iter().take_while(|&f| f <= &fork) {
            configs.insert(
                *fork,
                ForkConfig::new(*fork, Epoch::new(0), baseline_domain_type),
            );
        }
        Self {
            configs,
            network_name: network_name.to_string(),
        }
    }

    /// Get the network name.
    pub fn network_name(&self) -> &str {
        &self.network_name
    }

    /// Create a fork schedule from a map of forks to their raw configurations.
    ///
    /// This is primarily used when loading fork schedules from configuration files.
    ///
    /// # Notes
    ///
    /// Alan fork must be included in the input at epoch 0. This is because Alan
    /// is the network's genesis fork - all SSV networks started with the Alan
    /// fork active from the beginning. There is no "pre-Alan" state, so Alan
    /// cannot be scheduled at any epoch other than 0.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Alan fork is missing from configs
    /// - Alan fork is not at epoch 0
    /// - Fork epochs are not in chronological order
    pub fn from_fork_configs(
        raw_configs: BTreeMap<Fork, (Epoch, DomainType)>,
        network_name: &str,
    ) -> Result<Self, String> {
        // Alan must be present and at epoch 0
        match raw_configs.get(&Fork::Alan) {
            Some((epoch, _)) if *epoch != Epoch::new(0) => {
                return Err("Alan fork must be at epoch 0".to_string());
            }
            None => {
                return Err("Alan fork must be present in config".to_string());
            }
            _ => {}
        }

        // Validate chronological ordering - earlier forks must have < epochs
        // The exception is epoch 0, which may schedule multiple forks (to enable them at genesis)
        let mut prev_epoch: Option<u64> = None;
        for (fork, (epoch, _)) in &raw_configs {
            if let Some(prev) = prev_epoch
                && epoch.as_u64() <= prev
                && epoch.as_u64() != 0
            {
                return Err(format!(
                    "Fork {fork} at epoch {} is scheduled before an earlier fork at epoch {prev}",
                    epoch.as_u64()
                ));
            }
            prev_epoch = Some(epoch.as_u64());
        }

        // Convert raw configs to full ForkConfigs
        let configs = raw_configs
            .into_iter()
            .map(|(fork, (epoch, domain_type))| (fork, ForkConfig::new(fork, epoch, domain_type)))
            .collect();

        Ok(Self {
            configs,
            network_name: network_name.to_string(),
        })
    }

    /// Get the activation epoch for a fork, if scheduled.
    pub fn fork_epoch(&self, fork: Fork) -> Option<Epoch> {
        self.configs.get(&fork).map(|config| config.epoch)
    }

    /// Get the domain type for a fork, if scheduled.
    pub fn domain_type(&self, fork: Fork) -> Option<DomainType> {
        self.configs.get(&fork).map(|config| config.domain_type)
    }

    /// Get the full configuration for a fork, if scheduled.
    pub fn config(&self, fork: Fork) -> Option<&ForkConfig> {
        self.configs.get(&fork)
    }

    /// Get the currently active fork at the given epoch.
    ///
    /// Returns the latest fork that has activated by this epoch.
    pub fn active_fork(&self, epoch: Epoch) -> Fork {
        self.active_fork_config(epoch).fork
    }

    /// Get the configuration for the currently active fork at the given epoch.
    ///
    /// This is a convenience method that combines `active_fork` and `config`.
    pub fn active_fork_config(&self, epoch: Epoch) -> &ForkConfig {
        self.configs
            .values()
            .filter(|&config| epoch >= config.epoch)
            .max_by_key(|config| config.fork)
            .expect("constructors ensure there is at least one fork at epoch 0")
    }

    /// Get the next scheduled fork after the given epoch.
    pub fn next_fork_after(&self, epoch: Epoch) -> Option<(Fork, Epoch)> {
        self.configs
            .iter()
            .filter(|&(_, config)| config.epoch > epoch)
            .min_by_key(|(_, config)| config.epoch.as_u64())
            .map(|(fork, config)| (*fork, config.epoch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fork::ALAN_TOPIC_PREFIX;

    // Test constants
    const TEST_NETWORK: &str = "mainnet";
    const BASELINE_DOMAIN: DomainType = DomainType([0, 0, 0, 1]);
    const BOOLE_DOMAIN: DomainType = DomainType([0, 0, 0, 2]);

    fn schedule_with_boole(epoch: u64) -> ForkSchedule {
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Alan, (Epoch::new(0), BASELINE_DOMAIN));
        configs.insert(Fork::Boole, (Epoch::new(epoch), BOOLE_DOMAIN));
        ForkSchedule::from_fork_configs(configs, TEST_NETWORK).expect("valid test schedule")
    }

    #[test]
    fn test_new_schedule() {
        let schedule = ForkSchedule::new(Fork::Alan, BASELINE_DOMAIN, TEST_NETWORK);
        // Alan is active from epoch 0
        assert_eq!(schedule.active_fork(Epoch::new(0)), Fork::Alan);
        assert_eq!(schedule.active_fork(Epoch::new(100)), Fork::Alan);
        assert_eq!(schedule.domain_type(Fork::Alan), Some(BASELINE_DOMAIN));
        assert_eq!(schedule.network_name(), TEST_NETWORK);
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

        // Domain types
        assert_eq!(schedule.domain_type(Fork::Alan), Some(BASELINE_DOMAIN));
        assert_eq!(schedule.domain_type(Fork::Boole), Some(BOOLE_DOMAIN));
    }

    #[test]
    fn test_active_fork_config() {
        let schedule = schedule_with_boole(100);

        // Before Boole - should return Alan config
        let config = schedule.active_fork_config(Epoch::new(50));
        assert_eq!(config.fork, Fork::Alan);
        assert_eq!(config.domain_type, BASELINE_DOMAIN);

        // At Boole - should return Boole config
        let config = schedule.active_fork_config(Epoch::new(100));
        assert_eq!(config.fork, Fork::Boole);
        assert_eq!(config.domain_type, BOOLE_DOMAIN);
        assert_eq!(config.epoch, Epoch::new(100));

        // After Boole - should still return Boole config
        let config = schedule.active_fork_config(Epoch::new(200));
        assert_eq!(config.fork, Fork::Boole);
    }

    #[test]
    fn test_no_scheduled_boole() {
        let schedule = ForkSchedule::new(Fork::Alan, BASELINE_DOMAIN, TEST_NETWORK);
        assert_eq!(schedule.active_fork(Epoch::new(1000)), Fork::Alan);
        assert_eq!(schedule.fork_epoch(Fork::Boole), None);
        assert_eq!(schedule.domain_type(Fork::Boole), None);
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

    #[test]
    fn test_from_fork_configs_with_boole() {
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Alan, (Epoch::new(0), BASELINE_DOMAIN));
        configs.insert(Fork::Boole, (Epoch::new(100), BOOLE_DOMAIN));

        let schedule = ForkSchedule::from_fork_configs(configs, TEST_NETWORK).unwrap();
        assert_eq!(schedule.fork_epoch(Fork::Alan), Some(Epoch::new(0)));
        assert_eq!(schedule.fork_epoch(Fork::Boole), Some(Epoch::new(100)));
        assert_eq!(schedule.domain_type(Fork::Alan), Some(BASELINE_DOMAIN));
        assert_eq!(schedule.domain_type(Fork::Boole), Some(BOOLE_DOMAIN));
    }

    #[test]
    fn test_from_fork_configs_alan_only() {
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Alan, (Epoch::new(0), BASELINE_DOMAIN));

        let schedule = ForkSchedule::from_fork_configs(configs, TEST_NETWORK).unwrap();
        assert_eq!(schedule.fork_epoch(Fork::Alan), Some(Epoch::new(0)));
        assert_eq!(schedule.fork_epoch(Fork::Boole), None);
    }

    #[test]
    fn test_from_fork_configs_missing_alan() {
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Boole, (Epoch::new(100), BOOLE_DOMAIN));

        let result = ForkSchedule::from_fork_configs(configs, TEST_NETWORK);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be present"));
    }

    #[test]
    fn test_from_fork_configs_alan_wrong_epoch() {
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Alan, (Epoch::new(10), BASELINE_DOMAIN)); // Should be 0

        let result = ForkSchedule::from_fork_configs(configs, TEST_NETWORK);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be at epoch 0"));
    }

    #[test]
    fn test_topic_prefix_for_fork_alan() {
        let prefix = Fork::Alan.topic_prefix("mainnet");
        assert_eq!(prefix, ALAN_TOPIC_PREFIX);

        // Alan prefix is the same regardless of network
        let prefix_holesky = Fork::Alan.topic_prefix("holesky");
        assert_eq!(prefix_holesky, ALAN_TOPIC_PREFIX);
    }

    #[test]
    fn test_topic_prefix_for_fork_boole() {
        let prefix = Fork::Boole.topic_prefix("mainnet");
        assert_eq!(prefix, "/ssv/mainnet/boole/");

        let prefix_holesky = Fork::Boole.topic_prefix("holesky");
        assert_eq!(prefix_holesky, "/ssv/holesky/boole/");
    }
}
