use std::{
    collections::HashSet,
    task::{Context, Poll},
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

use crate::{Config, Enr};

pub mod connection;
pub mod discovery;
pub mod heartbeat;
pub mod types;

use connection::ConnectionManager;
use discovery::PeerDiscovery;
use heartbeat::HeartbeatManager;
pub use types::{ConnectActions, Event};

/// Main peer manager that coordinates all peer management functionality
pub struct PeerManager {
    peer_store: peer_store::Behaviour<MemoryStore<Enr>>,
    connection_manager: ConnectionManager,
    heartbeat_manager: HeartbeatManager,
    needed_subnets: HashSet<SubnetId>,
}

impl PeerManager {
    pub fn new(config: &Config) -> Self {
        let peer_store =
            peer_store::Behaviour::new(MemoryStore::new(memory_store::Config::default()));
        let connection_manager = ConnectionManager::new(config);
        let heartbeat_manager = HeartbeatManager::new();

        Self {
            peer_store,
            connection_manager,
            heartbeat_manager,
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
        )
    }

    /// Join subnet and dial peers for it
    pub fn join_subnet(&mut self, subnet_id: SubnetId) -> ConnectActions {
        PeerDiscovery::track_subnet_peers(
            subnet_id,
            &mut self.needed_subnets,
            self.peer_store.store(),
            &self.connection_manager,
        )
    }

    /// Perform heartbeat and return actions if needed
    pub fn heartbeat(&self) -> Option<ConnectActions> {
        HeartbeatManager::heartbeat(
            &self.needed_subnets,
            self.peer_store.store(),
            &self.connection_manager,
        )
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
            if let Some(actions) = self.heartbeat() {
                return Poll::Ready(ToSwarm::GenerateEvent(Event::ConnectActions(actions)));
            }
        }

        Poll::Pending
    }
}
