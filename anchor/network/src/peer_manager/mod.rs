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

use crate::{Config, Enr};

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
    peer_store: peer_store::Behaviour<MemoryStore<Enr>>,
    connection_manager: ConnectionManager,
    heartbeat_manager: HeartbeatManager,
    blocking_manager: BlockingManager,
    needed_subnets: HashSet<SubnetId>,
}

impl PeerManager {
    pub fn new(config: &Config, one_epoch_duration: Duration) -> Self {
        let peer_store =
            peer_store::Behaviour::new(MemoryStore::new(memory_store::Config::default()));
        let connection_manager = ConnectionManager::new(config);
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
    pub fn join_subnet(&mut self, subnet_id: SubnetId) -> ConnectActions {
        PeerDiscovery::track_subnet_peers(
            subnet_id,
            &mut self.needed_subnets,
            self.peer_store.store(),
            &self.connection_manager,
            self.blocking_manager.blocked_peers(),
        )
    }

    /// Perform heartbeat and return actions if needed
    pub fn heartbeat(&mut self) -> Option<ConnectActions> {
        info!(
            subnets = self.needed_subnets.len(),
            peers = self.connection_manager.connected.len(),
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
            FromSwarm::ConnectionEstablished(ConnectionEstablished { peer_id, .. }) => {
                self.connection_manager.on_connection_established(peer_id)
            }
            FromSwarm::ConnectionClosed(ConnectionClosed { peer_id, .. }) => {
                self.connection_manager.on_connection_closed(&peer_id)
            }
            _ => false,
        };

        // Update metrics if connection state changed
        self.connection_manager
            .update_metrics_if_changed(changed_connected);

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
