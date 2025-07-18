use std::{collections::HashSet, time::Duration};

use peer_store::memory_store::MemoryStore;
use subnet_service::SubnetId;
use tokio::time::{MissedTickBehavior, interval};
use tracing::info;

use super::{connection::ConnectionManager, discovery::PeerDiscovery, types::ConnectActions};
use crate::Enr;

/// Interval between heartbeat events in seconds
const HEARTBEAT_INTERVAL: u64 = 30;

/// Manages periodic heartbeat events and status reporting
pub struct HeartbeatManager {
    heartbeat: tokio::time::Interval,
}

impl HeartbeatManager {
    pub fn new() -> Self {
        let mut heartbeat = interval(Duration::from_secs(HEARTBEAT_INTERVAL));
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

        Self { heartbeat }
    }

    /// Check if it's time for a heartbeat
    pub fn poll_tick(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<tokio::time::Instant> {
        self.heartbeat.poll_tick(cx)
    }

    /// Log network status and check for needed peer actions
    pub fn heartbeat(
        needed_subnets: &HashSet<SubnetId>,
        peer_store: &MemoryStore<Enr>,
        connection_manager: &ConnectionManager,
    ) -> Option<ConnectActions> {
        info!(
            subnets = needed_subnets.len(),
            peers = connection_manager.connected.len(),
            "Network status"
        );

        PeerDiscovery::check_subnet_peers(needed_subnets, peer_store, connection_manager)
    }
}
