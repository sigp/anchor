use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
};

use discv5::libp2p_identity::PeerId;
use libp2p::{
    Multiaddr,
    connection_limits::{self, ConnectionLimits},
    core::{Endpoint, transport::PortUse},
    swarm::{ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour},
};
use peer_store::memory_store::MemoryStore;
use ssz_types::{Bitfield, length::Fixed, typenum::U128};
use subnet_service::SubnetId;
use thiserror::Error;

use crate::{Config, Enr, discovery, metrics::PEERS_CONNECTED};

/// A fraction of `target_peers` that we allow to connect to us in excess of
/// `target_peers`. For clarity, if `target_peers` is 50 and
/// PEER_EXCESS_FACTOR = 0.1 we allow 10% more nodes, i.e 55.
const PEER_EXCESS_FACTOR: f32 = 0.1;
/// A fraction of `target_peers` that if we get below, we start a discovery query to
/// reach our target. MIN_OUTBOUND_ONLY_FACTOR must be < TARGET_OUTBOUND_ONLY_FACTOR.
const MIN_OUTBOUND_ONLY_FACTOR: f32 = 0.2;
/// The fraction of extra peers beyond the PEER_EXCESS_FACTOR that we allow us to dial for when
/// requiring subnet peers. More specifically, if our target peer limit is 50, and our excess peer
/// limit is 55, and we are at 55 peers, the following parameter provisions a few more slots of
/// dialing priority peers we need for validator duties.
const PRIORITY_PEER_EXCESS: f32 = 0.2;

/// Minimum number of peers required per subnet
const MIN_PEERS_PER_SUBNET: usize = 6;

/// Specific peer connection errors
#[derive(Debug, Error)]
pub enum PeerConnectionError {
    #[error("peer not subscribed to any needed subnets")]
    MissingNeededSubnets,
}

/// Manages peer connections and connection limits
pub struct ConnectionManager {
    pub connection_limits: connection_limits::Behaviour,
    pub connected: HashSet<PeerId>,
    pub target_peers: usize,
    pub max_with_priority_peers: usize,
    // Map of observed gossipsub subscriptions per peer. Prefer this over ENR claims.
    observed_peer_subnets: HashMap<PeerId, Bitfield<Fixed<U128>>>,
}

impl ConnectionManager {
    pub fn new(config: &Config) -> Self {
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
            connection_limits,
            connected: HashSet::with_capacity(max_priority_peers),
            target_peers: config.target_peers,
            max_with_priority_peers: max_priority_peers,
            observed_peer_subnets: HashMap::new(),
        }
    }

    /// External update from gossipsub events about peer subscription state
    pub fn set_peer_subscribed(&mut self, peer: PeerId, subnet: SubnetId, subscribed: bool) {
        let entry = self.observed_peer_subnets.entry(peer).or_default();

        let idx = *subnet.deref() as usize;
        if idx < entry.len() {
            // Safe to ignore the result of `set` because we have already checked that `idx <
            // entry.len()`
            let _ = entry.set(idx, subscribed);
        }

        // If peer is now unsubscribed from all observed subnets, drop the entry to keep map small
        if !subscribed && !entry.iter().any(|b| b) {
            self.observed_peer_subnets.remove(&peer);
        }
    }

    /// Check if we should dial a peer based on current connection count
    pub fn should_dial_peer(
        &self,
        peer_id: &PeerId,
        peer_store: &MemoryStore<Enr>,
        needed_subnets: &HashSet<SubnetId>,
        blocked_peers: &HashSet<PeerId>,
    ) -> bool {
        // Don't dial blocked peers
        if blocked_peers.contains(peer_id) {
            return false;
        }

        self.connected.len() < self.target_peers
            || self.qualifies_for_priority(peer_id, peer_store, needed_subnets)
    }

    /// Check if a peer qualifies for priority dialing based on subnet requirements
    pub fn qualifies_for_priority(
        &self,
        peer_id: &PeerId,
        peer_store: &MemoryStore<Enr>,
        needed_subnets: &HashSet<SubnetId>,
    ) -> bool {
        let Some(subnets) = self.get_subnets_for_peer(peer_id, peer_store) else {
            return false;
        };
        let offered_subnets: HashSet<SubnetId> = subnets
            .iter()
            .enumerate()
            .filter_map(|(subnet, subscribed)| subscribed.then_some((subnet as u64).into()))
            .collect();

        let needed_and_offered = needed_subnets
            .intersection(&offered_subnets)
            .copied()
            .collect::<Vec<_>>();

        let counts = self.count_peers_for_subnets(&needed_and_offered, peer_store);
        for count in counts {
            if count < MIN_PEERS_PER_SUBNET {
                return true;
            }
        }
        false
    }

    /// Count how many connected peers are subscribed to each of the given subnets
    pub fn count_peers_for_subnets(
        &self,
        subnet_ids: &[SubnetId],
        peer_store: &MemoryStore<Enr>,
    ) -> Vec<usize> {
        let mut peer_subnet_counts = vec![0; subnet_ids.len()];
        for peer in self.connected.iter() {
            let Some(subnets) = self.get_subnets_for_peer(peer, peer_store) else {
                continue;
            };
            for (&subnet_id, count) in subnet_ids.iter().zip(&mut peer_subnet_counts) {
                let idx = *subnet_id.deref() as usize;
                if subnets.get(idx).unwrap_or(false) {
                    *count += 1;
                }
            }
        }
        peer_subnet_counts
    }

    /// Returns true if the peer appears to be subscribed to at least one of the needed subnets
    pub fn offers_any_needed(
        &self,
        peer: &PeerId,
        peer_store: &MemoryStore<Enr>,
        needed: &HashSet<SubnetId>,
    ) -> bool {
        if needed.is_empty() {
            return true;
        }
        let Some(bitfield) = self.get_subnets_for_peer(peer, peer_store) else {
            return false;
        };
        for subnet in needed {
            let idx = *subnet.deref() as usize;
            if bitfield.get(idx).unwrap_or(false) {
                return true;
            }
        }
        false
    }

    /// Get the subnets a peer is subscribed to, preferring observed gossipsub over ENR
    fn get_subnets_for_peer(
        &self,
        peer: &PeerId,
        peer_store: &MemoryStore<Enr>,
    ) -> Option<Bitfield<Fixed<U128>>> {
        if let Some(observed) = self.observed_peer_subnets.get(peer) {
            return Some(observed.clone());
        }
        // Fallback to ENR-advertised subnets
        let enr = peer_store.get_custom_data(peer)?;
        discovery::committee_bitfield(enr).ok()
    }

    /// Handle connection established event
    pub fn on_connection_established(&mut self, peer_id: PeerId) -> bool {
        self.connected.insert(peer_id)
    }

    /// Handle connection closed event
    pub fn on_connection_closed(&mut self, peer_id: &PeerId) -> bool {
        // Clear observed subscriptions on disconnect
        self.observed_peer_subnets.remove(peer_id);
        self.connected.remove(peer_id)
    }

    /// Update metrics if connection state changed
    pub fn update_metrics_if_changed(&self, changed: bool) {
        if changed {
            metrics::set_gauge(
                &PEERS_CONNECTED,
                self.connected.len().try_into().unwrap_or(0),
            );
        }
    }

    /// Handle pending inbound connection
    pub fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.connection_limits.handle_pending_inbound_connection(
            connection_id,
            local_addr,
            remote_addr,
        )
    }

    /// Shared post-processing for established connection results (inbound/outbound)
    fn finish_established_connection(
        &self,
        limit_result: Result<(), ConnectionDenied>,
        peer: PeerId,
        peer_store: &MemoryStore<Enr>,
        needed_subnets: &HashSet<SubnetId>,
    ) -> Result<(), ConnectionDenied> {
        match limit_result {
            Ok(()) => {
                if !self.offers_any_needed(&peer, peer_store, needed_subnets) {
                    return Err(ConnectionDenied::new(Box::new(
                        PeerConnectionError::MissingNeededSubnets,
                    )));
                }
                Ok(())
            }
            Err(denied) => {
                // TODO: deny if rejection reason is too many inbound connections
                // For this we need a way to access the denial kind, which is to be added to libp2p
                // https://github.com/sigp/anchor/issues/257
                if self.max_with_priority_peers > self.connected.len()
                    && self.qualifies_for_priority(&peer, peer_store, needed_subnets)
                {
                    Ok(())
                } else {
                    Err(denied)
                }
            }
        }
    }

    /// Handle established inbound connection with priority peer logic
    pub fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
        peer_store: &MemoryStore<Enr>,
        needed_subnets: &HashSet<SubnetId>,
    ) -> Result<(), ConnectionDenied> {
        let limit_result = self
            .connection_limits
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr)
            .map(|_| ()); // discard handler

        self.finish_established_connection(limit_result, peer, peer_store, needed_subnets)
    }

    /// Handle pending outbound connection
    pub fn handle_pending_outbound_connection(
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
        )
    }

    /// Handle established outbound connection with priority peer logic
    #[allow(clippy::too_many_arguments)]
    pub fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
        peer_store: &MemoryStore<Enr>,
        needed_subnets: &HashSet<SubnetId>,
    ) -> Result<(), ConnectionDenied> {
        let limit_result = self
            .connection_limits
            .handle_established_outbound_connection(
                connection_id,
                peer,
                addr,
                role_override,
                port_use,
            )
            .map(|_| ()); // discard handler

        self.finish_established_connection(limit_result, peer, peer_store, needed_subnets)
    }

    /// Handle swarm events related to connections
    pub fn on_swarm_event(&mut self, event: FromSwarm) {
        self.connection_limits.on_swarm_event(event);
    }
}
