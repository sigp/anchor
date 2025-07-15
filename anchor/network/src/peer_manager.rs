use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    task::{Context, Poll},
    time::Duration,
};

use discv5::{libp2p_identity::PeerId, multiaddr::Multiaddr};
use libp2p::{
    allow_block_list, connection_limits,
    connection_limits::ConnectionLimits,
    core::{Endpoint, transport::PortUse},
    swarm::{
        ConnectionClosed, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler,
        THandlerInEvent, THandlerOutEvent, ToSwarm,
        behaviour::ConnectionEstablished,
        dial_opts::{DialOpts, PeerCondition},
        dummy,
    },
};
use lighthouse_network::EnrExt;
use peer_store::{
    Store, memory_store,
    memory_store::{MemoryStore, PeerRecord},
};
use rand::seq::SliceRandom;
use ssz_types::{Bitfield, length::Fixed, typenum::U128};
use subnet_service::SubnetId;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info};

use crate::{Config, Enr, discovery, scoring::peer_score_config::RETAIN_SCORE_EPOCH_MULTIPLIER};

const MIN_PEERS_PER_SUBNET: usize = 6;

const PEER_OVERDIAL_FACTOR: usize = 2;

const HEARTBEAT_INTERVAL: u64 = 30;

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
    heartbeat: tokio::time::Interval,
    /// Block list behaviour for actual connection denial
    block_list: allow_block_list::Behaviour<allow_block_list::BlockedPeers>,
    /// Tracking when peers were blocked for automatic unblocking
    blocked_peers_info: HashMap<PeerId, tokio::time::Instant>,
    /// One epoch duration for calculating retain_score timeout
    one_epoch_duration: Duration,
}

impl PeerManager {
    pub fn new(config: &Config, one_epoch_duration: Duration) -> Self {
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

        let mut heartbeat = interval(Duration::from_secs(HEARTBEAT_INTERVAL));
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

        Self {
            peer_store,
            connection_limits,
            connected: HashSet::with_capacity(max_priority_peers),
            needed_subnets: HashSet::new(),
            target_peers: config.target_peers,
            max_with_priority_peers: max_priority_peers,
            heartbeat,
            block_list: allow_block_list::Behaviour::<allow_block_list::BlockedPeers>::default(),
            blocked_peers_info: HashMap::new(),
            one_epoch_duration,
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

    /// Block a peer based on poor gossipsub score
    pub fn block_peer_for_poor_score(&mut self, peer_id: PeerId) {
        if self.block_list.block_peer(peer_id) {
            self.blocked_peers_info
                .insert(peer_id, tokio::time::Instant::now());
            debug!(?peer_id, "Blocked peer due to poor gossipsub score");
        }
    }

    /// Unblock a peer and remove from tracking
    pub fn unblock_peer(&mut self, peer_id: PeerId) -> bool {
        let was_removed = self.block_list.unblock_peer(peer_id);
        if was_removed {
            self.blocked_peers_info.remove(&peer_id);
            debug!(?peer_id, "Unblocked peer after retain_score duration");
        }
        was_removed
    }

    /// Get list of currently blocked peers
    pub fn blocked_peers(&self) -> &HashSet<PeerId> {
        self.block_list.blocked_peers()
    }

    /// Check and unblock peers that have been blocked long enough
    pub fn check_and_unblock_expired_peers(&mut self) {
        let retain_score_duration = self.one_epoch_duration * RETAIN_SCORE_EPOCH_MULTIPLIER;
        let now = tokio::time::Instant::now();

        let peers_to_unblock: Vec<PeerId> = self
            .blocked_peers_info
            .iter()
            .filter_map(|(&peer_id, &blocked_at)| {
                if now.duration_since(blocked_at) >= retain_score_duration {
                    Some(peer_id)
                } else {
                    None
                }
            })
            .collect();

        for peer_id in peers_to_unblock {
            self.unblock_peer(peer_id);
        }
    }

    /// Join subnet and dial peers for it. Returns true if we need to discover peers for it
    pub fn join_subnet(&mut self, subnet_id: SubnetId) -> ConnectActions {
        self.needed_subnets.insert(subnet_id);

        let mut actions = ConnectActions::none();
        self.determine_actions_for_subnets(&mut actions, &[subnet_id]);

        actions
    }

    pub fn determine_actions_for_subnets(
        &self,
        actions: &mut ConnectActions,
        subnets: &[SubnetId],
    ) {
        let peer_counts = self.count_peers_for_subnets(subnets);
        let mut subnet_needs = subnets
            .iter()
            .zip(peer_counts)
            .filter_map(|(subnet, count)| {
                let need = MIN_PEERS_PER_SUBNET.saturating_sub(count) * PEER_OVERDIAL_FACTOR;
                (need != 0).then_some((*subnet, need))
            })
            .collect::<HashMap<_, _>>();

        for (peer, record) in self.candidate_peers() {
            let Some(enr) = record.get_custom_data() else {
                continue;
            };

            let subnets = discovery::committee_bitfield(enr).unwrap_or_default();

            let mut relevant = false;
            for subnet in subnets
                .iter()
                .enumerate()
                .filter_map(|(subnet, subscribed)| {
                    subscribed.then_some(SubnetId::new(subnet as u64))
                })
            {
                let Entry::Occupied(mut need) = subnet_needs.entry(subnet) else {
                    continue;
                };
                relevant = true;
                *need.get_mut() -= 1;
                if need.get() == &0 {
                    need.remove();
                }
            }

            if relevant {
                actions.dial.push(self.peer_to_dial_opts(*peer));
            }
        }

        actions.discover.extend(subnet_needs.into_keys());
    }

    pub fn heartbeat(&mut self) -> Option<ConnectActions> {
        info!(
            subnets = self.needed_subnets.len(),
            peers = self.connected.len(),
            blocked_peers = self.blocked_peers_info.len(),
            "Network status"
        );

        // Check and unblock peers that have been blocked long enough
        self.check_and_unblock_expired_peers();

        let mut actions = ConnectActions::none();
        self.determine_actions_for_subnets(
            &mut actions,
            &self.needed_subnets.iter().copied().collect::<Vec<_>>(),
        );

        (!actions.discover.is_empty() || !actions.dial.is_empty()).then_some(actions)
    }

    fn candidate_peers(&self) -> Vec<(&PeerId, &PeerRecord<Enr>)> {
        let mut peers = self
            .peer_store
            .store()
            .record_iter()
            .filter(|(peer, record)| {
                !self.connected.contains(peer) && record.addresses().next().is_some()
            })
            .collect::<Vec<_>>();
        peers.shuffle(&mut rand::rng());
        peers
    }

    fn get_subnets_for_peer(&self, peer: &PeerId) -> Option<Bitfield<Fixed<U128>>> {
        let enr = self.peer_store.store().get_custom_data(peer)?;
        discovery::committee_bitfield(enr).ok()
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
            .copied()
            .collect::<Vec<_>>();

        let counts = self.count_peers_for_subnets(&needed_and_offered);
        for count in counts {
            if count < MIN_PEERS_PER_SUBNET {
                return true;
            }
        }
        false
    }

    fn count_peers_for_subnets(&self, subnet_ids: &[SubnetId]) -> Vec<usize> {
        let mut peer_subnet_counts = vec![0; subnet_ids.len()];
        for peer in self.connected.iter() {
            let Some(subnets) = self.get_subnets_for_peer(peer) else {
                continue;
            };
            for (&subnet_id, count) in subnet_ids.iter().zip(&mut peer_subnet_counts) {
                if subnets.get(*subnet_id as usize).unwrap_or(false) {
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

#[derive(Debug)]
pub struct ConnectActions {
    pub dial: Vec<DialOpts>,
    pub discover: Vec<SubnetId>,
}

impl ConnectActions {
    fn none() -> Self {
        ConnectActions {
            dial: vec![],
            discover: vec![],
        }
    }
}

#[derive(Debug)]
pub struct HeartbeatEvent {
    pub connect_actions: Option<ConnectActions>,
    pub check_peer_scores: bool,
}

#[derive(Debug)]
pub enum Event {
    PeerStore(peer_store::Event<memory_store::Event>),
    PeerManagerHeartbeat(HeartbeatEvent),
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
        // Check block list first
        self.block_list.handle_pending_inbound_connection(
            connection_id,
            local_addr,
            remote_addr,
        )?;

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
        // Check block list first
        self.block_list.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        )?;

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
        // For this we need a way to access the denial kind, which is to be added to libp2p
        // https://github.com/sigp/anchor/issues/257

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
        // Check block list first
        self.block_list.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?;

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
        // Check block list first
        self.block_list.handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        )?;

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
        // Handle block list events first
        self.block_list.on_swarm_event(event);

        // `changed` is `true` only when the set actually grew or shrank.
        let changed_connected = match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished { peer_id, .. }) => {
                self.connected.insert(peer_id)
            }
            FromSwarm::ConnectionClosed(ConnectionClosed { peer_id, .. }) => {
                self.connected.remove(&peer_id)
            }
            _ => false,
        };

        if changed_connected {
            lighthouse_network::metrics::set_gauge(
                &lighthouse_network::metrics::PEERS_CONNECTED,
                self.connected.len().try_into().unwrap_or(0),
            );
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
        // Check block list events first (although it typically doesn't generate events)
        if let Poll::Ready(e) = self.block_list.poll(cx) {
            return Poll::Ready(e.map_out(|never| match never {}));
        }

        // Check connection limits
        if let Poll::Ready(e) = self.connection_limits.poll(cx) {
            return Poll::Ready(e.map_out(|never| match never {}));
        }

        // Check peer store events
        if let Poll::Ready(e) = self.peer_store.poll(cx) {
            return Poll::Ready(e.map_out(Event::PeerStore));
        }

        // Check heartbeat timer
        if self.heartbeat.poll_tick(cx).is_ready() {
            let connect_actions = self.heartbeat();
            return Poll::Ready(ToSwarm::GenerateEvent(Event::PeerManagerHeartbeat(
                HeartbeatEvent {
                    connect_actions,
                    check_peer_scores: true,
                },
            )));
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use libp2p::identity::Keypair;

    use super::*;
    use crate::Config;

    /// Test helper to create a test PeerManager
    fn create_test_peer_manager() -> PeerManager {
        let config = Config {
            target_peers: 10,
            ..Config::default()
        };
        let one_epoch_duration = Duration::from_secs(384); // 32 slots * 12 seconds
        PeerManager::new(&config, one_epoch_duration)
    }

    /// Test helper to create a test peer ID
    fn create_test_peer_id() -> PeerId {
        let keypair = Keypair::generate_ed25519();
        keypair.public().to_peer_id()
    }

    #[tokio::test(start_paused = true)]
    async fn test_peer_blocking_for_poor_score() {
        let mut peer_manager = create_test_peer_manager();
        let peer_id = create_test_peer_id();

        // Initially, peer should not be blocked
        assert!(!peer_manager.blocked_peers().contains(&peer_id));
        assert!(peer_manager.blocked_peers_info.is_empty());

        // Block the peer for poor score
        peer_manager.block_peer_for_poor_score(peer_id);

        // Verify peer is now blocked
        assert!(peer_manager.blocked_peers().contains(&peer_id));
        assert!(peer_manager.blocked_peers_info.contains_key(&peer_id));

        // Verify the block time was recorded (should be at the current paused time)
        let block_time = peer_manager.blocked_peers_info.get(&peer_id).unwrap();
        let expected_time = tokio::time::Instant::now();
        assert_eq!(*block_time, expected_time);
    }

    #[tokio::test(start_paused = true)]
    async fn test_peer_unblocking_after_timeout() {
        let mut peer_manager = create_test_peer_manager();
        let peer_id = create_test_peer_id();

        // Block the peer
        peer_manager.block_peer_for_poor_score(peer_id);
        assert!(peer_manager.blocked_peers().contains(&peer_id));

        // Advance time beyond the retain_score period
        let retain_score_duration = peer_manager.one_epoch_duration * RETAIN_SCORE_EPOCH_MULTIPLIER;
        tokio::time::advance(retain_score_duration + Duration::from_secs(1)).await;

        // Check and unblock expired peers
        peer_manager.check_and_unblock_expired_peers();

        // Verify peer is now unblocked
        assert!(!peer_manager.blocked_peers().contains(&peer_id));
        assert!(!peer_manager.blocked_peers_info.contains_key(&peer_id));
    }

    #[tokio::test(start_paused = true)]
    async fn test_peer_not_unblocked_before_timeout() {
        let mut peer_manager = create_test_peer_manager();
        let peer_id = create_test_peer_id();

        // Block the peer
        peer_manager.block_peer_for_poor_score(peer_id);
        assert!(peer_manager.blocked_peers().contains(&peer_id));

        // Advance time but not enough to trigger unblocking
        let retain_score_duration = peer_manager.one_epoch_duration * RETAIN_SCORE_EPOCH_MULTIPLIER;
        tokio::time::advance(retain_score_duration - Duration::from_secs(10)).await;

        // Check and unblock expired peers
        peer_manager.check_and_unblock_expired_peers();

        // Verify peer is still blocked
        assert!(peer_manager.blocked_peers().contains(&peer_id));
        assert!(peer_manager.blocked_peers_info.contains_key(&peer_id));
    }

    #[tokio::test(start_paused = true)]
    async fn test_multiple_peers_blocking_and_unblocking() {
        let mut peer_manager = create_test_peer_manager();
        let peer_id_1 = create_test_peer_id();
        let peer_id_2 = create_test_peer_id();
        let peer_id_3 = create_test_peer_id();

        // Block peer_1 first
        peer_manager.block_peer_for_poor_score(peer_id_1);

        // Advance time a bit
        tokio::time::advance(Duration::from_secs(100)).await;

        // Block peer_2 and peer_3
        peer_manager.block_peer_for_poor_score(peer_id_2);
        peer_manager.block_peer_for_poor_score(peer_id_3);

        // Verify all are blocked
        assert_eq!(peer_manager.blocked_peers().len(), 3);
        assert_eq!(peer_manager.blocked_peers_info.len(), 3);

        // Advance time enough to unblock only peer_1 (it was blocked earlier)
        let retain_score_duration = peer_manager.one_epoch_duration * RETAIN_SCORE_EPOCH_MULTIPLIER;
        tokio::time::advance(retain_score_duration - Duration::from_secs(50)).await;

        // Check and unblock expired peers
        peer_manager.check_and_unblock_expired_peers();

        // Only peer_1 should be unblocked
        assert!(!peer_manager.blocked_peers().contains(&peer_id_1));
        assert!(peer_manager.blocked_peers().contains(&peer_id_2));
        assert!(peer_manager.blocked_peers().contains(&peer_id_3));
        assert_eq!(peer_manager.blocked_peers().len(), 2);
        assert_eq!(peer_manager.blocked_peers_info.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn test_manual_unblock_peer() {
        let mut peer_manager = create_test_peer_manager();
        let peer_id = create_test_peer_id();

        // Block the peer
        peer_manager.block_peer_for_poor_score(peer_id);
        assert!(peer_manager.blocked_peers().contains(&peer_id));

        // Manually unblock the peer
        let was_unblocked = peer_manager.unblock_peer(peer_id);
        assert!(was_unblocked);

        // Verify peer is now unblocked
        assert!(!peer_manager.blocked_peers().contains(&peer_id));
        assert!(!peer_manager.blocked_peers_info.contains_key(&peer_id));

        // Trying to unblock again should return false
        let was_unblocked_again = peer_manager.unblock_peer(peer_id);
        assert!(!was_unblocked_again);
    }
}
