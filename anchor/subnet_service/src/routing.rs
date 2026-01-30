//! Topic routing for fork-aware message publishing and subscription.
//!
//! This module is the single source of truth for determining which gossipsub topics
//! messages should be published to or subscribed to, based on the fork schedule.
//!
//! Per SIP-43, routing decisions are slot-based:
//! - Messages are published to topics based on their slot's active fork
//! - During fork transitions, both old and new topics may be active

use std::sync::Arc;

use fork::{Fork, ForkSchedule};
use ssv_types::domain_type::DomainType;
use types::{Epoch, Slot};

use crate::SubnetId;

/// Topic router - single source of truth for fork-aware topic routing.
///
/// Handles all decisions about which topics to use for publishing and subscribing,
/// based on the fork schedule and message slots.
#[derive(Clone)]
pub struct TopicRouter {
    fork_schedule: Arc<ForkSchedule>,
    slots_per_epoch: u64,
}

impl TopicRouter {
    /// Create a new topic router.
    pub fn new(fork_schedule: Arc<ForkSchedule>, slots_per_epoch: u64) -> Self {
        Self {
            fork_schedule,
            slots_per_epoch,
        }
    }

    /// Get the number of slots per epoch.
    pub fn slots_per_epoch(&self) -> u64 {
        self.slots_per_epoch
    }

    /// Get the active fork at a given epoch.
    pub fn active_fork(&self, epoch: Epoch) -> Fork {
        self.fork_schedule.active_fork(epoch)
    }

    /// Get the active fork at a given slot.
    pub fn active_fork_at_slot(&self, slot: Slot) -> Fork {
        let epoch = slot.epoch(self.slots_per_epoch);
        self.fork_schedule.active_fork(epoch)
    }

    /// Get the domain type for the fork active at a given epoch.
    pub fn domain_type_for_epoch(&self, epoch: Epoch) -> Option<DomainType> {
        let fork = self.fork_schedule.active_fork(epoch);
        self.fork_schedule.domain_type(fork)
    }

    /// Create a topic string for a given subnet and epoch.
    ///
    /// This determines the correct fork for the epoch and creates the full topic
    /// string using that fork's topic prefix.
    ///
    /// Per SIP-43, publishing should use the topic corresponding to the message's
    /// slot, not necessarily the current fork.
    pub fn topic_for_subnet_at_epoch(&self, subnet: SubnetId, epoch: Epoch) -> String {
        let config = self.fork_schedule.active_fork_config(epoch);
        format!("{}{}", config.topic_prefix, *subnet)
    }

    /// Create a topic string for a given subnet and slot.
    ///
    /// Convenience method that converts the slot to an epoch and delegates to
    /// `topic_for_subnet_at_epoch`.
    pub fn topic_for_subnet_at_slot(&self, subnet: SubnetId, slot: Slot) -> String {
        let epoch = slot.epoch(self.slots_per_epoch);
        self.topic_for_subnet_at_epoch(subnet, epoch)
    }

    /// Get the fork schedule reference.
    ///
    /// This is exposed for components that need direct access to fork schedule
    /// for more complex queries (e.g., preparation windows, fork epochs).
    pub fn fork_schedule(&self) -> &ForkSchedule {
        &self.fork_schedule
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const TEST_NETWORK: &str = "mainnet";
    const BASELINE_DOMAIN: DomainType = DomainType([0, 0, 0, 1]);
    const BOOLE_DOMAIN: DomainType = DomainType([0, 0, 0, 2]);
    const SLOTS_PER_EPOCH: u64 = 32;
    const BOOLE_FORK_EPOCH: u64 = 100;

    fn create_test_router() -> TopicRouter {
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Alan, (Epoch::new(0), BASELINE_DOMAIN));
        configs.insert(Fork::Boole, (Epoch::new(BOOLE_FORK_EPOCH), BOOLE_DOMAIN));
        let fork_schedule =
            ForkSchedule::from_fork_configs(configs, TEST_NETWORK).expect("valid test schedule");
        TopicRouter::new(Arc::new(fork_schedule), SLOTS_PER_EPOCH)
    }

    #[test]
    fn test_active_fork_before_boole() {
        let router = create_test_router();
        assert_eq!(router.active_fork(Epoch::new(50)), Fork::Alan);
        assert_eq!(router.active_fork(Epoch::new(99)), Fork::Alan);
    }

    #[test]
    fn test_active_fork_at_and_after_boole() {
        let router = create_test_router();
        assert_eq!(router.active_fork(Epoch::new(100)), Fork::Boole);
        assert_eq!(router.active_fork(Epoch::new(200)), Fork::Boole);
    }

    #[test]
    fn test_active_fork_at_slot() {
        let router = create_test_router();

        // Slot in epoch 99 (pre-Boole)
        let pre_fork_slot = Slot::new(99 * SLOTS_PER_EPOCH + 15);
        assert_eq!(router.active_fork_at_slot(pre_fork_slot), Fork::Alan);

        // Slot in epoch 100 (Boole)
        let post_fork_slot = Slot::new(100 * SLOTS_PER_EPOCH + 5);
        assert_eq!(router.active_fork_at_slot(post_fork_slot), Fork::Boole);
    }

    #[test]
    fn test_topic_for_subnet_uses_correct_prefix() {
        let router = create_test_router();
        let subnet = SubnetId::from(42u64);

        // Pre-Boole epoch - should use Alan prefix
        let pre_fork_topic = router.topic_for_subnet_at_epoch(subnet, Epoch::new(50));
        assert_eq!(pre_fork_topic, "ssv.v2.42");

        // Post-Boole epoch - should use Boole prefix
        let post_fork_topic = router.topic_for_subnet_at_epoch(subnet, Epoch::new(100));
        assert_eq!(post_fork_topic, "/ssv/mainnet/boole/42");
    }

    #[test]
    fn test_topic_for_subnet_at_slot() {
        let router = create_test_router();
        let subnet = SubnetId::from(10u64);

        // Pre-fork slot
        let pre_fork_slot = Slot::new(50 * SLOTS_PER_EPOCH);
        assert_eq!(
            router.topic_for_subnet_at_slot(subnet, pre_fork_slot),
            "ssv.v2.10"
        );

        // Post-fork slot
        let post_fork_slot = Slot::new(100 * SLOTS_PER_EPOCH);
        assert_eq!(
            router.topic_for_subnet_at_slot(subnet, post_fork_slot),
            "/ssv/mainnet/boole/10"
        );
    }

    #[test]
    fn test_domain_type_for_epoch() {
        let router = create_test_router();

        // Pre-Boole
        assert_eq!(
            router.domain_type_for_epoch(Epoch::new(50)),
            Some(BASELINE_DOMAIN)
        );

        // Post-Boole
        assert_eq!(
            router.domain_type_for_epoch(Epoch::new(100)),
            Some(BOOLE_DOMAIN)
        );
    }
}
