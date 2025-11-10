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

use crate::{ClientType, PeerInfo, discovery, metrics::PEERS_CONNECTED};

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
    // Track inbound vs outbound connection counts
    inbound_count: usize,
    outbound_count: usize,
}

impl ConnectionManager {
    /// Create connection limits for a given target peer count.
    fn create_connection_limits(target_peers: usize) -> connection_limits::Behaviour {
        let limits = ConnectionLimits::default()
            .with_max_pending_incoming(Some(5))
            .with_max_pending_outgoing(Some(16))
            .with_max_established_incoming(Some(
                (target_peers as f32 * (1.0 + PEER_EXCESS_FACTOR - MIN_OUTBOUND_ONLY_FACTOR)).ceil()
                    as u32,
            ))
            .with_max_established_outgoing(Some(
                (target_peers as f32 * (1.0 + PEER_EXCESS_FACTOR)).ceil() as u32,
            ))
            .with_max_established(Some(
                (target_peers as f32 * (1.0 + PEER_EXCESS_FACTOR)).ceil() as u32,
            ))
            .with_max_established_per_peer(Some(1));

        connection_limits::Behaviour::new(limits)
    }

    /// Initialize ConnectionManager with a target peer count.
    pub fn new(target_peers: usize) -> Self {
        let connection_limits = Self::create_connection_limits(target_peers);

        let max_priority_peers = (target_peers as f32
            * (1.0 + PEER_EXCESS_FACTOR + PRIORITY_PEER_EXCESS))
            .ceil() as usize;

        Self {
            connection_limits,
            connected: HashSet::with_capacity(max_priority_peers),
            target_peers,
            max_with_priority_peers: max_priority_peers,
            observed_peer_subnets: HashMap::new(),
            inbound_count: 0,
            outbound_count: 0,
        }
    }

    /// Update the target peer count and recalculate connection limits.
    ///
    /// This is called by PeerManager when dynamic peer calculation is enabled
    /// and the number of active subnets changes.
    pub fn set_target_peers(&mut self, new_target: usize) {
        if self.target_peers == new_target {
            return;
        }

        tracing::debug!(
            old_target = self.target_peers,
            new_target,
            "Updating target peer count"
        );

        self.target_peers = new_target;

        self.max_with_priority_peers =
            (new_target as f32 * (1.0 + PEER_EXCESS_FACTOR + PRIORITY_PEER_EXCESS)).ceil() as usize;

        self.connection_limits = Self::create_connection_limits(new_target);
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
        peer_store: &MemoryStore<PeerInfo>,
        needed_subnets: &HashSet<SubnetId>,
        blocked_peers: &HashSet<PeerId>,
    ) -> bool {
        // Don't dial blocked peers
        if blocked_peers.contains(peer_id) {
            return false;
        }

        // Don't dial connected peers
        if self.connected.contains(peer_id) {
            return false;
        }

        self.connected.len() < self.target_peers
            || self.qualifies_for_priority_connection(peer_id, peer_store, needed_subnets)
    }

    /// Check if a peer qualifies for priority dialing based on subnet requirements.
    /// This uses ENR fallback because it's used during connection decisions where we haven't
    /// observed gossipsub behavior yet.
    pub fn qualifies_for_priority_connection(
        &self,
        peer_id: &PeerId,
        peer_store: &MemoryStore<PeerInfo>,
        needed_subnets: &HashSet<SubnetId>,
    ) -> bool {
        let Some(subnets) = self.get_peer_subnets_with_enr_fallback(peer_id, peer_store) else {
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

        let counts = self.count_observed_peers_for_subnets(&needed_and_offered);
        for count in counts {
            if count < MIN_PEERS_PER_SUBNET {
                return true;
            }
        }
        false
    }

    /// Count how many connected peers are actually subscribed to each subnet based on observed
    /// gossipsub. This only counts peers we've observed via gossipsub, no ENR fallback.
    /// Used for making decisions about existing connections and subnet health.
    pub fn count_observed_peers_for_subnets(&self, subnet_ids: &[SubnetId]) -> Vec<usize> {
        let mut peer_subnet_counts = vec![0; subnet_ids.len()];
        for peer in self.connected.iter() {
            let Some(subnets) = self.get_peer_subnets_observed_only(peer) else {
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

    /// Check if a peer offers any needed subnets based only on observed gossipsub subscriptions.
    /// Used for disconnect decisions where we don't trust ENR claims.
    pub fn peer_offers_needed_subnets_observed_only(
        &self,
        peer: &PeerId,
        needed: &HashSet<SubnetId>,
    ) -> bool {
        if needed.is_empty() {
            return true;
        }

        // Only use observed subscriptions, no ENR fallback
        let Some(observed) = self.observed_peer_subnets.get(peer) else {
            return false;
        };

        self.bitfield_offers_any_subnet(observed, needed)
    }

    /// Check if a peer offers any needed subnets, using ENR as fallback.
    /// Used for connection decisions where we haven't observed gossipsub behavior yet.
    pub fn peer_offers_needed_subnets_with_enr_fallback(
        &self,
        peer: &PeerId,
        peer_store: &MemoryStore<PeerInfo>,
        needed: &HashSet<SubnetId>,
    ) -> bool {
        if needed.is_empty() {
            return true;
        }

        let Some(bitfield) = self.get_peer_subnets_with_enr_fallback(peer, peer_store) else {
            // Most peers that connect to us, that we have never seen, we will not know of their
            // ENR. We should allow all incoming peers and then later reject them if
            // they pose no use to us.
            return true;
        };

        // If we have seen this peer before, and we know it isn't useful, then we can reject it.
        self.bitfield_offers_any_subnet(&bitfield, needed)
    }

    /// Helper to check if a bitfield offers any of the needed subnets
    fn bitfield_offers_any_subnet(
        &self,
        bitfield: &Bitfield<Fixed<U128>>,
        needed: &HashSet<SubnetId>,
    ) -> bool {
        for subnet in needed {
            let idx = *subnet.deref() as usize;
            if bitfield.get(idx).unwrap_or(false) {
                return true;
            }
        }
        false
    }

    /// Get subnets a peer claims to support from observed gossipsub only.
    fn get_peer_subnets_observed_only(&self, peer: &PeerId) -> Option<Bitfield<Fixed<U128>>> {
        self.observed_peer_subnets.get(peer).cloned()
    }

    /// Get subnets a peer claims to support, with ENR fallback.
    fn get_peer_subnets_with_enr_fallback(
        &self,
        peer: &PeerId,
        peer_store: &MemoryStore<PeerInfo>,
    ) -> Option<Bitfield<Fixed<U128>>> {
        self.get_peer_subnets_observed_only(peer).or_else(|| {
            // Fallback to ENR
            peer_store
                .get_custom_data(peer)?
                .enr
                .as_ref()
                .and_then(|enr| discovery::committee_bitfield(enr).ok())
        })
    }

    /// Handle connection established event
    pub fn on_connection_established(&mut self, peer_id: PeerId, is_outbound: bool) -> bool {
        // Initialize with empty bitfield to indicate we're now observing this peer
        // If they never subscribe to anything, we'll know they offer no subnets
        self.observed_peer_subnets.entry(peer_id).or_default();

        // Track connection direction counter
        let is_new = self.connected.insert(peer_id);
        if is_new {
            if is_outbound {
                self.outbound_count += 1;
            } else {
                self.inbound_count += 1;
            }
        }

        is_new
    }

    /// Handle connection closed event
    pub fn on_connection_closed(&mut self, peer_id: &PeerId, was_outbound: bool) -> bool {
        // Clear observed subscriptions on disconnect
        self.observed_peer_subnets.remove(peer_id);

        // Decrement appropriate counter based on direction
        let was_connected = self.connected.remove(peer_id);
        if was_connected {
            if was_outbound {
                self.outbound_count = self.outbound_count.saturating_sub(1);
            } else {
                self.inbound_count = self.inbound_count.saturating_sub(1);
            }
        }

        was_connected
    }

    /// Get the number of inbound connections
    pub fn inbound_count(&self) -> usize {
        self.inbound_count
    }

    /// Get the number of outbound connections
    pub fn outbound_count(&self) -> usize {
        self.outbound_count
    }

    /// Update metrics if connection state changed
    pub fn update_metrics_if_changed(&self, changed: bool, peer_store: &MemoryStore<PeerInfo>) {
        if changed {
            metrics::set_gauge(
                &PEERS_CONNECTED,
                self.connected.len().try_into().unwrap_or(0),
            );

            let mut anchor_count = 0;
            let mut go_ssv_count = 0;
            let mut unknown_count = 0;

            // Count all connected peers by client type
            for peer_id in self.connected.iter() {
                if let Some(data) = peer_store.get_custom_data(peer_id) {
                    match data.client_type {
                        Some(ClientType::Anchor) => anchor_count += 1,
                        Some(ClientType::GoSSV) => go_ssv_count += 1,
                        None => unknown_count += 1,
                    }
                } else {
                    unknown_count += 1;
                }
            }

            metrics::set_gauge_vec(&crate::metrics::PEERS_BY_CLIENT, &["anchor"], anchor_count);
            metrics::set_gauge_vec(&crate::metrics::PEERS_BY_CLIENT, &["go-ssv"], go_ssv_count);
            metrics::set_gauge_vec(
                &crate::metrics::PEERS_BY_CLIENT,
                &["unknown"],
                unknown_count,
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
        peer_store: &MemoryStore<PeerInfo>,
        needed_subnets: &HashSet<SubnetId>,
    ) -> Result<(), ConnectionDenied> {
        match limit_result {
            Ok(()) => {
                // For new connections, we can be lenient and use ENR fallback
                // since we haven't had time to observe gossipsub behavior yet
                if !self.peer_offers_needed_subnets_with_enr_fallback(
                    &peer,
                    peer_store,
                    needed_subnets,
                ) {
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
                    && self.qualifies_for_priority_connection(&peer, peer_store, needed_subnets)
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
        peer_store: &MemoryStore<PeerInfo>,
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
        peer_store: &MemoryStore<PeerInfo>,
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
