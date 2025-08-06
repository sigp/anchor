use std::time::Duration;

use tokio::time::{MissedTickBehavior, interval};

use super::types::ConnectActions;

/// Interval between heartbeat events in seconds
const HEARTBEAT_INTERVAL: u64 = 30;

/// Heartbeat event containing both connection actions and peer score check signal
#[derive(Debug)]
pub struct Event {
    pub connect_actions: Option<ConnectActions>,
    pub check_peer_scores: bool,
}

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
}
