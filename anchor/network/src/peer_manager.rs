use crate::{Config, Enr};
use discv5::libp2p_identity::PeerId;
use discv5::multiaddr::Multiaddr;
use libp2p::connection_limits::ConnectionLimits;
use libp2p::core::transport::PortUse;
use libp2p::core::Endpoint;
use libp2p::peer_store::memory_store::MemoryStore;
use libp2p::peer_store::{memory_store, Store};
use libp2p::swarm::behaviour::ConnectionEstablished;
use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use libp2p::swarm::{
    dummy, ConnectionClosed, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler,
    THandlerInEvent, THandlerOutEvent, ToSwarm,
};
use libp2p::{connection_limits, peer_store};
use lighthouse_network::EnrExt;
use ssz::Decode;
use ssz_types::length::Fixed;
use ssz_types::typenum::U128;
use ssz_types::{BitVector, Bitfield};
use std::collections::HashSet;
use std::task::{Context, Poll};
use subnet_tracker::SubnetId;
use tracing::debug;

const MIN_PEERS_PER_SUBNET: usize = 3;

/// A fraction of `PeerManager::target_peers` that we allow to connect to us in excess of
/// `PeerManager::target_peers`. For clarity, if `PeerManager::target_peers` is 50 and
/// PEER_EXCESS_FACTOR = 0.1 we allow 10% more nodes, i.e 55.
const PEER_EXCESS_FACTOR: f32 = 0.1;
/// A fraction of `PeerManager::target_peers` that if we get below, we start a discovery query to
/// reach our target. MIN_OUTBOUND_ONLY_FACTOR must be < TARGET_OUTBOUND_ONLY_FACTOR.
const MIN_OUTBOUND_ONLY_FACTOR: f32 = 0.2;
/// The fraction of extra peers beyond the PEER_EXCESS_FACTOR that we allow us to dial for when
/// requiring subnet peers. More specifically, if our target peer limit is 50, and our excess peer
/// limit is 55, and we are at 55 peers, the following parameter provisions a few more slots of
/// dialing priority peers we need for validator duties.
const PRIORITY_PEER_EXCESS: f32 = 0.2;

pub struct PeerManager {
    peer_store: peer_store::Behaviour<MemoryStore<Enr>>,
    connection_limits: connection_limits::Behaviour,
    connected: HashSet<PeerId>,
    needed_subnets: HashSet<SubnetId>,
    target_peers: usize,
    max_with_priority_peers: usize,
}

impl PeerManager {
    pub fn new(config: &Config) -> Self {
        let peer_store =
            peer_store::Behaviour::new(MemoryStore::new(memory_store::Config::default()));

        let connection_limits = {
            let limits = ConnectionLimits::default()
                .with_max_pending_incoming(Some(5))
                .with_max_pending_outgoing(Some(16))
                .with_max_established_incoming(Some(
                    (config.target_peers as f32
                        * (1.0 + PEER_EXCESS_FACTOR - MIN_OUTBOUND_ONLY_FACTOR))
                        .ceil() as u32,
                ))
                .with_max_established_outgoing(Some(
                    (config.target_peers as f32 * (1.0 + PEER_EXCESS_FACTOR)).ceil() as u32,
                ))
                .with_max_established(Some(
                    (config.target_peers as f32 * (1.0 + PEER_EXCESS_FACTOR)).ceil() as u32,
                ))
                .with_max_established_per_peer(Some(1));

            connection_limits::Behaviour::new(limits)
        };

        let max_priority_peers = (config.target_peers as f32
            * (1.0 + PEER_EXCESS_FACTOR + PRIORITY_PEER_EXCESS))
            .ceil() as usize;

        Self {
            peer_store,
            connection_limits,
            connected: HashSet::with_capacity(max_priority_peers),
            needed_subnets: HashSet::new(),
            target_peers: config.target_peers,
            max_with_priority_peers: max_priority_peers,
        }
    }

    /// report a discovered peer, and return dial opts if we want to dial it
    pub fn discovered_peer(&mut self, enr: Enr) -> Option<DialOpts> {
        let id = enr.peer_id();

        let store = self.peer_store.store_mut();
        // first, make the store aware of it
        for multiaddr in enr.multiaddr() {
            store.update_address(&id, &multiaddr);
        }
        store.insert_custom_data(&id, enr.clone());

        let dial = self.connected.len() < self.target_peers || self.qualifies_for_priority(&id);

        // dial
        dial.then(|| self.peer_to_dial_opts(id))
    }

    /// Join subnet and dial peers for it. Returns true if we need to discover peers for it
    pub fn join_subnet(&mut self, subnet_id: SubnetId) -> SubnetConnectActions {
        self.needed_subnets.insert(subnet_id);

        let peer_count = self.count_peers_for_subnets(&[&subnet_id])[0];
        let mut missing_peers = MIN_PEERS_PER_SUBNET.saturating_sub(peer_count);

        if missing_peers == 0 {
            return SubnetConnectActions::none();
        }

        let mut dial = vec![];
        for (peer, record) in self.peer_store.store_mut().record_iter() {
            let Some(enr) = record.get_custom_data() else {
                continue;
            };

            // todo make getting this easier with our own "EnrExt"
            let subnets = enr
                .get_decodable::<[u8; 16]>("subnets")
                .and_then(|result| result.ok())
                .and_then(|array| BitVector::<U128>::from_ssz_bytes(&array).ok())
                .unwrap_or_default();

            if let Ok(true) = subnets.get(*subnet_id as usize) {
                dial.push(*peer);
                missing_peers -= 1;
                if missing_peers == 0 {
                    break;
                }
            }
        }

        SubnetConnectActions {
            discover: missing_peers != 0,
            dial,
        }
    }

    fn get_subnets_for_peer(&self, peer: &PeerId) -> Option<Bitfield<Fixed<U128>>> {
        let enr = self.peer_store.store().get_custom_data(peer)?;
        let subnets = enr.get_decodable::<[u8; 16]>("subnets")?.ok()?;
        Bitfield::from_ssz_bytes(&subnets).ok()
    }

    fn qualifies_for_priority(&self, peer: &PeerId) -> bool {
        let Some(subnets) = self.get_subnets_for_peer(peer) else {
            return false;
        };
        let offered_subnets: HashSet<SubnetId> = subnets
            .iter()
            .enumerate()
            .filter_map(|(subnet, subscribed)| subscribed.then_some((subnet as u64).into()))
            .collect();

        let needed_and_offered = self
            .needed_subnets
            .intersection(&offered_subnets)
            .collect::<Vec<_>>();

        let counts = self.count_peers_for_subnets(&needed_and_offered);
        for count in counts {
            if count < MIN_PEERS_PER_SUBNET {
                return true;
            }
        }
        false
    }

    fn count_peers_for_subnets(&self, subnet_ids: &[&SubnetId]) -> Vec<usize> {
        let mut peer_subnet_counts = vec![0; subnet_ids.len()];
        for peer in self.connected.iter() {
            let Some(subnets) = self.get_subnets_for_peer(peer) else {
                continue;
            };
            for (&subnet_id, count) in subnet_ids.iter().zip(&mut peer_subnet_counts) {
                if subnets.get(**subnet_id as usize).unwrap_or(false) {
                    *count += 1;
                }
            }
        }
        peer_subnet_counts
    }

    fn peer_to_dial_opts(&self, peer: PeerId) -> DialOpts {
        let addresses = self
            .peer_store
            .store()
            .addresses_of_peer(&peer)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        debug!(?peer, ?addresses, "Let's dial!");
        DialOpts::peer_id(peer)
            .condition(PeerCondition::DisconnectedAndNotDialing)
            .addresses(addresses)
            .build()
    }
}

pub struct SubnetConnectActions {
    pub dial: Vec<PeerId>,
    pub discover: bool,
}

impl SubnetConnectActions {
    fn none() -> Self {
        SubnetConnectActions {
            dial: vec![],
            discover: false,
        }
    }
}

impl NetworkBehaviour for PeerManager {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = peer_store::Event<memory_store::Event>;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        // we call the peer store here first to remember the peer regardless of whether we accept a
        // connection with it right now.
        self.peer_store.handle_pending_inbound_connection(
            connection_id,
            local_addr,
            remote_addr,
        )?;
        self.connection_limits.handle_pending_inbound_connection(
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
        self.peer_store.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        )?;
        let limit_result = self
            .connection_limits
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr);

        let Err(denied) = limit_result else {
            return Ok(dummy::ConnectionHandler);
        };

        // TODO: deny if rejection reason is too many inbound connections
        // for this we need a way to access the denial kind, which is to be added to libp2p

        if self.max_with_priority_peers > self.connected.len() && self.qualifies_for_priority(&peer)
        {
            Ok(dummy::ConnectionHandler)
        } else {
            Err(denied)
        }
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        self.connection_limits.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?;
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
        self.peer_store.handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        )?;
        let limit_result = self
            .connection_limits
            .handle_established_outbound_connection(
                connection_id,
                peer,
                addr,
                role_override,
                port_use,
            );

        let Err(denied) = limit_result else {
            return Ok(dummy::ConnectionHandler);
        };

        if self.max_with_priority_peers > self.connected.len() && self.qualifies_for_priority(&peer)
        {
            Ok(dummy::ConnectionHandler)
        } else {
            Err(denied)
        }
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished { peer_id, .. }) => {
                self.connected.insert(peer_id);
            }
            FromSwarm::ConnectionClosed(ConnectionClosed { peer_id, .. }) => {
                self.connected.remove(&peer_id);
            }
            _ => {}
        }
        self.connection_limits.on_swarm_event(event);
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
        if let Poll::Ready(e) = self.connection_limits.poll(cx) {
            return Poll::Ready(e.map_out(|never| match never {}));
        }
        if let Poll::Ready(e) = self.peer_store.poll(cx) {
            return Poll::Ready(e);
        }
        Poll::Pending
    }
}
