use std::{
    collections::HashSet,
    task::{Context, Poll},
    time::Duration,
};

use discv5::libp2p_identity::PeerId;
use libp2p::{
    Multiaddr,
    core::{Endpoint, transport::PortUse},
    swarm::{
        ConnectionClosed, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler,
        THandlerInEvent, THandlerOutEvent, ToSwarm, behaviour::ConnectionEstablished,
        dial_opts::DialOpts, dummy,
    },
};
use peer_store::memory_store::{self, MemoryStore};
use subnet_service::SubnetId;
use tracing::info;

use crate::{ClientType, Config, Enr, peer_manager::types::PeerInfo};

pub mod blocking;
pub mod connection;
pub mod discovery;
pub mod heartbeat;
pub mod types;

use blocking::BlockingManager;
use connection::ConnectionManager;
use discovery::PeerDiscovery;
use heartbeat::HeartbeatManager;
pub use types::{ConnectActions, Event};

/// Main peer manager that coordinates all peer management functionality
pub struct PeerManager {
    peer_store: peer_store::Behaviour<MemoryStore<PeerInfo>>,
    connection_manager: ConnectionManager,
    heartbeat_manager: HeartbeatManager,
    blocking_manager: BlockingManager,
    needed_subnets: HashSet<SubnetId>,
}

/// Base number of peers to maintain when no subnets are active.
/// This value is copied from the go-ssv implementation.
const BASE_PEER_COUNT: usize = 60;

/// Additional peers to add for each active subnet.
/// This value is copied from the go-ssv implementation.
const PEERS_PER_SUBNET: usize = 3;

/// Maximum number of peers to maintain, regardless of subnet count.
/// This value is copied from the go-ssv implementation.
const MAX_PEER_COUNT: usize = 150;

impl PeerManager {
    /// Calculate target peer count based on active subnet count.
    ///
    /// Formula: base 60 peers + 3 peers per active subnet, capped at 150 maximum.
    /// This calculation is arbitrary and copied from the go-ssv implementation for
    /// compatibility with the existing network.
    pub fn calculate_target_peers(active_subnet_count: usize) -> usize {
        use std::cmp::min;
        min(
            BASE_PEER_COUNT + active_subnet_count * PEERS_PER_SUBNET,
            MAX_PEER_COUNT,
        )
    }

    /// Create a new PeerManager with the given configuration.
    ///
    /// # Arguments
    /// * `config` - Network configuration (may contain user-provided target_peers)
    /// * `one_epoch_duration` - Duration of one epoch for blocking calculations
    pub fn new(config: &Config, one_epoch_duration: Duration) -> Self {
        let peer_store =
            peer_store::Behaviour::new(MemoryStore::new(memory_store::Config::default()));

        // Determine target_peers: use user's value if provided, otherwise start with base count.
        // When dynamic (None), target_peers will be updated as subnets are joined via
        // subnet_service.
        let target_peers = config.target_peers.unwrap_or(BASE_PEER_COUNT);

        let connection_manager = ConnectionManager::new(target_peers);
        let heartbeat_manager = HeartbeatManager::new();
        let blocking_manager = BlockingManager::new(one_epoch_duration);

        Self {
            peer_store,
            connection_manager,
            heartbeat_manager,
            blocking_manager,
            needed_subnets: HashSet::new(),
        }
    }

    /// Report a discovered peer and return dial options if we want to dial it
    pub fn report_discovered_peer(&mut self, enr: Enr) -> Option<DialOpts> {
        PeerDiscovery::process_discovered_peer(
            enr,
            self.peer_store.store_mut(),
            &self.connection_manager,
            &self.needed_subnets,
            self.blocking_manager.blocked_peers(),
        )
    }

    /// Join subnet and dial peers for it
    pub fn join_subnet(
        &mut self,
        subnet_id: SubnetId,
        is_dynamic_target_peers: bool,
    ) -> ConnectActions {
        let actions = PeerDiscovery::track_subnet_peers(
            subnet_id,
            &mut self.needed_subnets,
            self.peer_store.store(),
            &self.connection_manager,
            self.blocking_manager.blocked_peers(),
        );

        if is_dynamic_target_peers {
            let new_target = Self::calculate_target_peers(self.needed_subnets.len());
            self.connection_manager.set_target_peers(new_target);
        }

        actions
    }

    /// Leave subnet
    pub fn leave_subnet(&mut self, subnet_id: SubnetId, is_dynamic_target_peers: bool) {
        self.needed_subnets.remove(&subnet_id);

        if is_dynamic_target_peers {
            let new_target = Self::calculate_target_peers(self.needed_subnets.len());
            self.connection_manager.set_target_peers(new_target);
        }
    }

    /// Perform heartbeat and return actions if needed
    pub fn heartbeat(&mut self) -> Option<ConnectActions> {
        info!(
            subnets = self.needed_subnets.len(),
            peers = self.connection_manager.connected.len(),
            inbound = self.connection_manager.inbound_count(),
            outbound = self.connection_manager.outbound_count(),
            blocked_peers = self.blocking_manager.blocked_peers_count(),
            "Network status"
        );

        // Check and unblock peers that have been blocked long enough
        self.blocking_manager.check_and_unblock_expired_peers();

        // Check if any subnets need more peers and return dial/discovery actions
        PeerDiscovery::check_subnet_peers(
            &self.needed_subnets,
            self.peer_store.store(),
            &self.connection_manager,
            self.blocking_manager.blocked_peers(),
        )
    }

    /// Block a peer and track timestamp for automatic unblocking
    pub fn block_peer(&mut self, peer_id: PeerId) -> bool {
        self.blocking_manager.block_peer(peer_id)
    }

    /// Unblock a peer, allowing it to connect again
    pub fn unblock_peer(&mut self, peer_id: PeerId) -> bool {
        self.blocking_manager.unblock_peer(peer_id)
    }

    /// Get the list of currently blocked peers
    pub fn blocked_peers(&self) -> &std::collections::HashSet<PeerId> {
        self.blocking_manager.blocked_peers()
    }

    /// Get the target number of peers
    pub fn target_peers(&self) -> usize {
        self.connection_manager.target_peers
    }

    /// Get the current number of connected peers
    pub fn connected_peers(&self) -> usize {
        self.connection_manager.connected.len()
    }

    /// Get the number of active subnets we're subscribed to
    pub fn active_subnet_count(&self) -> usize {
        self.needed_subnets.len()
    }

    /// Get the number of inbound connections
    pub fn inbound_peers(&self) -> usize {
        self.connection_manager.inbound_count()
    }

    /// Get the number of outbound connections
    pub fn outbound_peers(&self) -> usize {
        self.connection_manager.outbound_count()
    }

    /// Get the set of subnets we need peers for
    pub fn needed_subnets(&self) -> &HashSet<SubnetId> {
        &self.needed_subnets
    }

    /// Update observed gossipsub subscription state for a peer
    pub fn set_peer_subscription(&mut self, peer: PeerId, subnet: SubnetId, subscribed: bool) {
        self.connection_manager
            .set_peer_subscribed(peer, subnet, subscribed);
    }

    /// Handle a completed handshake by updating peer client type and triggering metrics update.
    pub fn handle_handshake_completed(&mut self, peer_id: PeerId, node_version: String) {
        let client_type = ClientType::from(node_version);

        // Update client type in peer store
        if let Some(peer_info) = self.peer_store.store_mut().get_custom_data_mut(&peer_id) {
            peer_info.set_client_type(client_type);
        } else {
            self.peer_store.store_mut().insert_custom_data(
                &peer_id,
                PeerInfo {
                    enr: None,
                    client_type: Some(client_type),
                },
            );
        }

        // Trigger metric recalculation
        self.connection_manager
            .update_metrics_if_changed(true, self.peer_store.store());
    }

    /// Returns true if a connected peer should be disconnected because it doesn't offer any needed
    /// subnets based on observed gossipsub subscriptions (no ENR fallback)
    pub fn should_disconnect_due_to_subnets(&self, peer: &PeerId) -> bool {
        !self
            .connection_manager
            .peer_offers_needed_subnets_observed_only(peer, &self.needed_subnets)
    }

    /// Collect peers that should be disconnected due to not offering any needed subnets
    pub fn peers_to_disconnect_due_to_subnets(&self) -> Vec<PeerId> {
        self.connection_manager
            .connected
            .iter()
            .filter(|p| self.should_disconnect_due_to_subnets(p))
            .cloned()
            .collect()
    }
}

impl NetworkBehaviour for PeerManager {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = Event;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        // Check block list first - delegate to blocking manager
        self.blocking_manager.handle_pending_inbound_connection(
            connection_id,
            local_addr,
            remote_addr,
        )?;

        // Handle peer store first to remember the peer
        self.peer_store.handle_pending_inbound_connection(
            connection_id,
            local_addr,
            remote_addr,
        )?;

        // Then handle connection limits
        self.connection_manager.handle_pending_inbound_connection(
            connection_id,
            local_addr,
            remote_addr,
        )
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        // Check block list first - delegate to blocking manager
        self.blocking_manager
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr)?;

        // Handle peer store first
        self.peer_store.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        )?;

        // Then handle connection limits with priority logic
        self.connection_manager
            .handle_established_inbound_connection(
                connection_id,
                peer,
                local_addr,
                remote_addr,
                self.peer_store.store(),
                &self.needed_subnets,
            )?;

        Ok(dummy::ConnectionHandler)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        // Check block list first - delegate to blocking manager
        self.blocking_manager.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?;

        // Handle connection limits first
        self.connection_manager.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?;

        // Then handle peer store
        self.peer_store.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        // Check block list first - delegate to blocking manager
        self.blocking_manager
            .handle_established_outbound_connection(
                connection_id,
                peer,
                addr,
                role_override,
                port_use,
            )?;

        // Handle peer store first
        self.peer_store.handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        )?;

        // Then handle connection limits with priority logic
        self.connection_manager
            .handle_established_outbound_connection(
                connection_id,
                peer,
                addr,
                role_override,
                port_use,
                self.peer_store.store(),
                &self.needed_subnets,
            )?;

        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        // Handle connection state changes
        let changed_connected = match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id, endpoint, ..
            }) => {
                let is_outbound = endpoint.is_dialer();
                self.connection_manager
                    .on_connection_established(peer_id, is_outbound)
            }
            FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id, endpoint, ..
            }) => {
                let was_outbound = endpoint.is_dialer();
                self.connection_manager
                    .on_connection_closed(&peer_id, was_outbound)
            }
            _ => false,
        };

        // Update metrics if connection state changed
        self.connection_manager
            .update_metrics_if_changed(changed_connected, self.peer_store.store());

        // Delegate to sub-components
        self.blocking_manager.on_swarm_event(event);
        self.connection_manager.on_swarm_event(event);
        self.peer_store.on_swarm_event(event);
    }

    fn on_connection_handler_event(
        &mut self,
        _peer_id: PeerId,
        _connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {}
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        // Check blocking manager events first and forward them
        if let Poll::Ready(e) = self.blocking_manager.poll(cx) {
            return Poll::Ready(
                e.map_out(|never| match never {})
                    .map_in(|never| match never {}),
            );
        }

        // Check connection limits
        if let Poll::Ready(e) = self.connection_manager.connection_limits.poll(cx) {
            return Poll::Ready(e.map_out(|never| match never {}));
        }

        // Check peer store events
        if let Poll::Ready(e) = self.peer_store.poll(cx) {
            return Poll::Ready(e.map_out(Event::PeerStore));
        }

        // Check heartbeat timer
        if self.heartbeat_manager.poll_tick(cx).is_ready() {
            let connect_actions = self.heartbeat();
            return Poll::Ready(ToSwarm::GenerateEvent(Event::Heartbeat(heartbeat::Event {
                connect_actions,
                check_peer_scores: true,
            })));
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that calculate_target_peers correctly implements the formula:
    /// BASE_PEER_COUNT + PEERS_PER_SUBNET * subnets, capped at MAX_PEER_COUNT
    #[test]
    fn test_calculate_target_peers_formula() {
        // Base case: 0 subnets
        assert_eq!(PeerManager::calculate_target_peers(0), BASE_PEER_COUNT);

        // Linear growth: BASE_PEER_COUNT + PEERS_PER_SUBNET * subnets
        assert_eq!(
            PeerManager::calculate_target_peers(1),
            BASE_PEER_COUNT + PEERS_PER_SUBNET
        );
        assert_eq!(
            PeerManager::calculate_target_peers(5),
            BASE_PEER_COUNT + 5 * PEERS_PER_SUBNET
        );
        assert_eq!(
            PeerManager::calculate_target_peers(10),
            BASE_PEER_COUNT + 10 * PEERS_PER_SUBNET
        );
        assert_eq!(
            PeerManager::calculate_target_peers(20),
            BASE_PEER_COUNT + 20 * PEERS_PER_SUBNET
        );

        // At cap boundary (60 + 30 * 3 = 150)
        assert_eq!(PeerManager::calculate_target_peers(30), MAX_PEER_COUNT);

        // Above cap - should be capped at MAX_PEER_COUNT
        assert_eq!(PeerManager::calculate_target_peers(31), MAX_PEER_COUNT);
        assert_eq!(PeerManager::calculate_target_peers(50), MAX_PEER_COUNT);
        assert_eq!(PeerManager::calculate_target_peers(100), MAX_PEER_COUNT);
    }
}
