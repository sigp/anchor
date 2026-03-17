use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
};

use discv5::libp2p_identity::PeerId;
use fork::{Fork, ForkLifecycle};
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
use tokio::sync::watch;

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

/// Observed gossipsub subnet state for a peer.
///
/// This distinguishes "no observed evidence yet" from "observed and empty", which
/// controls whether ENR fallback is allowed.
#[derive(Debug, Clone)]
enum ObservedPeerSubnets {
    /// No gossipsub subscription events observed for this peer yet.
    Unknown,
    /// At least one subscription event observed and currently no subnet bits are set.
    KnownEmpty,
    /// At least one subnet bit observed as set (tracked per fork).
    Known(HashMap<Fork, Bitfield<Fixed<U128>>>),
}

/// Manages peer connections and connection limits
pub struct ConnectionManager {
    pub connection_limits: connection_limits::Behaviour,
    pub connected: HashSet<PeerId>,
    pub target_peers: usize,
    pub max_with_priority_peers: usize,
    // Observed gossipsub subscriptions per peer with explicit state:
    // - Unknown: no observed subscription evidence yet (ENR fallback allowed)
    // - KnownEmpty: observed but currently no subscribed subnets
    // - Known: per-fork bitmaps
    //
    // Per-fork tracking prevents unsubscribing from one fork's topic from clearing
    // the bit for the same subnet on another fork's topic.
    // See: https://github.com/sigp/anchor/issues/818
    observed_peer_subnets: HashMap<PeerId, ObservedPeerSubnets>,
    // Fork lifecycle receiver for fork-aware peer selection.
    // After the grace period ends, only peers on the current fork are considered useful.
    lifecycle_rx: watch::Receiver<ForkLifecycle>,
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

    /// Initialize ConnectionManager with a target peer count and fork lifecycle receiver.
    pub fn new(target_peers: usize, lifecycle_rx: watch::Receiver<ForkLifecycle>) -> Self {
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
            lifecycle_rx,
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

    /// External update from gossipsub events about peer subscription state.
    ///
    /// Subscriptions are tracked per fork so that unsubscribing from one fork's topic
    /// (e.g., `ssv.v2.42`) does not clear the subnet bit if the peer is still
    /// subscribed via another fork's topic (e.g., `/ssv/mainnet/boole/42`).
    pub fn set_peer_subscribed(
        &mut self,
        peer: PeerId,
        fork: Fork,
        subnet: SubnetId,
        subscribed: bool,
    ) {
        let idx = *subnet.deref() as usize;
        let bitfield_len = Bitfield::<Fixed<U128>>::default().len();

        if idx >= bitfield_len {
            tracing::warn!(
                %peer,
                subnet = idx,
                max = bitfield_len,
                "Subnet ID exceeds bitfield capacity"
            );
            return;
        }

        let state = self
            .observed_peer_subnets
            .entry(peer)
            .or_insert(ObservedPeerSubnets::Unknown);

        if subscribed {
            if !matches!(state, ObservedPeerSubnets::Known(_)) {
                *state = ObservedPeerSubnets::Known(HashMap::new());
            }

            if let ObservedPeerSubnets::Known(fork_map) = state {
                let bitfield = fork_map.entry(fork).or_default();
                let _ = bitfield.set(idx, true);
            }
            return;
        }

        match state {
            ObservedPeerSubnets::Unknown => {
                // We have now observed explicit subscription state, but no positive bits.
                *state = ObservedPeerSubnets::KnownEmpty;
            }
            ObservedPeerSubnets::KnownEmpty => {}
            ObservedPeerSubnets::Known(fork_map) => {
                if let Some(bitfield) = fork_map.get_mut(&fork) {
                    let _ = bitfield.set(idx, false);
                    if bitfield.is_zero() {
                        fork_map.remove(&fork);
                    }
                }

                if fork_map.is_empty() {
                    *state = ObservedPeerSubnets::KnownEmpty;
                }
            }
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

        // Don't dial already-connected peers
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
        // Read the fork lifecycle once for the entire iteration rather than per-peer.
        let lifecycle = self.lifecycle_rx.borrow().clone();
        let mut peer_subnet_counts = vec![0; subnet_ids.len()];
        for peer in self.connected.iter() {
            let Some(subnets) = self.get_peer_subnets_for_lifecycle(peer, &lifecycle) else {
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
        let Some(observed) = self.get_peer_subnets_observed_only(peer) else {
            return false;
        };

        self.bitfield_offers_any_subnet(&observed, needed)
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

    /// Get subnets a peer claims to support from observed gossipsub subscriptions.
    ///
    /// Convenience wrapper that reads the fork lifecycle once per call. For bulk
    /// operations over many peers, prefer [`get_peer_subnets_for_lifecycle`] with
    /// a pre-read lifecycle value to avoid repeated lock acquisition.
    fn get_peer_subnets_observed_only(&self, peer: &PeerId) -> Option<Bitfield<Fixed<U128>>> {
        let lifecycle = self.lifecycle_rx.borrow().clone();
        self.get_peer_subnets_for_lifecycle(peer, &lifecycle)
    }

    /// Fork-aware peer subnet lookup with a pre-read lifecycle value.
    ///
    /// In `Normal` state (post-grace-period), only the current fork's bitmap is
    /// returned. During `WarmUp` or `GracePeriod`, bitmaps from both relevant
    /// forks are aggregated (union). This prevents peers subscribed only to a
    /// defunct fork from appearing useful for subnet coverage.
    fn get_peer_subnets_for_lifecycle(
        &self,
        peer: &PeerId,
        lifecycle: &ForkLifecycle,
    ) -> Option<Bitfield<Fixed<U128>>> {
        match self.observed_peer_subnets.get(peer)? {
            ObservedPeerSubnets::Unknown => None,
            ObservedPeerSubnets::KnownEmpty => Some(Bitfield::default()),
            ObservedPeerSubnets::Known(fork_map) => match lifecycle {
                ForkLifecycle::Normal { current, .. } => Some(
                    fork_map
                        .get(&current.fork)
                        .cloned()
                        .unwrap_or_else(Bitfield::default),
                ),
                ForkLifecycle::WarmUp { .. } | ForkLifecycle::GracePeriod { .. } => {
                    Some(Self::aggregate_fork_bitmaps(fork_map))
                }
            },
        }
    }

    /// OR all per-fork bitmaps together into a single aggregate bitfield.
    fn aggregate_fork_bitmaps(
        fork_map: &HashMap<Fork, Bitfield<Fixed<U128>>>,
    ) -> Bitfield<Fixed<U128>> {
        fork_map
            .values()
            .fold(Bitfield::default(), |acc, bf| acc.union(bf))
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
        // Initialize as Unknown: no observed gossipsub evidence yet.
        self.observed_peer_subnets
            .entry(peer_id)
            .or_insert(ObservedPeerSubnets::Unknown);

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
    #[expect(clippy::too_many_arguments)]
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

#[cfg(test)]
mod tests {
    use fork::ForkConfig;
    use types::Epoch;

    use super::*;

    // ==================== Test constants ====================

    const TARGET_PEERS: usize = 50;
    const ALAN_DOMAIN: ssv_types::domain_type::DomainType =
        ssv_types::domain_type::DomainType([0, 0, 0, 1]);
    const BOOLE_DOMAIN: ssv_types::domain_type::DomainType =
        ssv_types::domain_type::DomainType([0, 0, 0, 2]);

    fn alan_config() -> ForkConfig {
        ForkConfig::new(Fork::Alan, Epoch::new(0), ALAN_DOMAIN)
    }

    fn boole_config() -> ForkConfig {
        ForkConfig::new(Fork::Boole, Epoch::new(100), BOOLE_DOMAIN)
    }

    // Subnet IDs used across tests. Each has a distinct role to aid readability.
    const SUBNET_A: u64 = 5;
    const SUBNET_B: u64 = 42;
    const SUBNET_C: u64 = 10;
    const SUBNET_D: u64 = 20;
    const SUBNET_E: u64 = 99;
    const SUBNET_UNSUBSCRIBED: u64 = 30;

    fn subnet(id: u64) -> SubnetId {
        SubnetId::new(id)
    }

    // ==================== Helper functions ====================

    /// Creates a `ConnectionManager` with default target peers for testing.
    /// Defaults to `ForkLifecycle::Normal { current: Alan }` for backward compatibility.
    fn create_test_manager() -> ConnectionManager {
        create_test_manager_with_lifecycle(ForkLifecycle::Normal {
            current: alan_config(),
        })
    }

    /// Creates a `ConnectionManager` with a specific fork lifecycle state.
    fn create_test_manager_with_lifecycle(lifecycle: ForkLifecycle) -> ConnectionManager {
        let (_tx, rx) = watch::channel(lifecycle);
        ConnectionManager::new(TARGET_PEERS, rx)
    }

    /// Connects a peer to the manager (adds to `connected` and marks observed
    /// subscription state as Unknown), returning the generated `PeerId`.
    fn connect_random_peer(mgr: &mut ConnectionManager) -> PeerId {
        let peer = PeerId::random();
        mgr.on_connection_established(peer, /* is_outbound = */ true);
        peer
    }

    /// Returns the aggregated (union across forks) bitfield for a peer, or `None`.
    fn aggregated_bitfield(
        mgr: &ConnectionManager,
        peer: &PeerId,
    ) -> Option<Bitfield<Fixed<U128>>> {
        mgr.get_peer_subnets_observed_only(peer)
    }

    /// Checks whether a specific subnet bit is set in the aggregated bitfield.
    fn is_subnet_set(mgr: &ConnectionManager, peer: &PeerId, subnet: u64) -> bool {
        aggregated_bitfield(mgr, peer)
            .map(|bf| bf.get(subnet as usize).unwrap_or(false))
            .unwrap_or(false)
    }

    // ==================== Single fork subscribe/unsubscribe ====================

    #[test]
    fn test_set_peer_subscribed_single_fork_subscribe() {
        // Arrange
        let mut mgr = create_test_manager();
        let peer = connect_random_peer(&mut mgr);

        // Act
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);

        // Assert
        assert!(
            is_subnet_set(&mgr, &peer, SUBNET_A),
            "Subnet should be set after subscribing on Alan fork"
        );
    }

    #[test]
    fn test_set_peer_subscribed_single_fork_unsubscribe() {
        // Arrange
        let mut mgr = create_test_manager();
        let peer = connect_random_peer(&mut mgr);
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);

        // Act
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), false);

        // Assert
        assert!(
            !is_subnet_set(&mgr, &peer, SUBNET_A),
            "Subnet should be cleared after unsubscribing on Alan fork"
        );
    }

    // ==================== Multi-fork bug scenario (issue #818) ====================
    //
    // These tests use GracePeriod lifecycle because they test cross-fork aggregation
    // behavior, which only occurs during WarmUp or GracePeriod states.

    /// Creates a `ConnectionManager` in GracePeriod lifecycle for multi-fork tests.
    fn create_grace_period_manager() -> ConnectionManager {
        create_test_manager_with_lifecycle(ForkLifecycle::GracePeriod {
            current: boole_config(),
            previous: alan_config(),
        })
    }

    /// Regression test for https://github.com/sigp/anchor/issues/818
    ///
    /// When a peer is subscribed to the same subnet on two different forks,
    /// unsubscribing from one fork must NOT clear the subnet bit in the
    /// aggregated view because the other fork still holds it.
    #[test]
    fn test_unsubscribe_one_fork_retains_subnet_from_other_fork() {
        // Arrange: grace period aggregates across forks
        let mut mgr = create_grace_period_manager();
        let peer = connect_random_peer(&mut mgr);

        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_B), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_B), true);

        // Act: unsubscribe from Alan only
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_B), false);

        // Assert: subnet should still be set because Boole holds it
        assert!(
            is_subnet_set(&mgr, &peer, SUBNET_B),
            "Subnet must remain set when Boole fork still subscribes to it"
        );
    }

    #[test]
    fn test_unsubscribe_both_forks_clears_subnet() {
        // Arrange: grace period aggregates across forks
        let mut mgr = create_grace_period_manager();
        let peer = connect_random_peer(&mut mgr);

        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_B), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_B), true);

        // Act: unsubscribe from both forks
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_B), false);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_B), false);

        // Assert: subnet should now be cleared
        assert!(
            !is_subnet_set(&mgr, &peer, SUBNET_B),
            "Subnet must be cleared after unsubscribing from all forks"
        );
    }

    // ==================== Aggregation across forks ====================

    #[test]
    fn test_aggregation_unions_subnets_across_forks() {
        // Arrange: grace period aggregates across forks
        let mut mgr = create_grace_period_manager();
        let peer = connect_random_peer(&mut mgr);

        // Act: subscribe to different subnets on different forks
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_C), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_D), true);

        // Assert: aggregated view should contain both bits
        let bf = aggregated_bitfield(&mgr, &peer).expect("peer should have an aggregated bitfield");
        assert!(
            bf.get(SUBNET_C as usize).unwrap_or(false),
            "Alan subnet must be present in aggregated bitfield"
        );
        assert!(
            bf.get(SUBNET_D as usize).unwrap_or(false),
            "Boole subnet must be present in aggregated bitfield"
        );
        assert!(
            !bf.get(SUBNET_UNSUBSCRIBED as usize).unwrap_or(false),
            "Unsubscribed subnet should NOT be present"
        );
    }

    // ==================== Explicit observed-state transitions ====================

    #[test]
    fn test_full_unsubscribe_marks_peer_known_empty() {
        // Arrange
        let mut mgr = create_test_manager();
        let peer = connect_random_peer(&mut mgr);
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_E), true);

        // Act: unsubscribe from everything
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), false);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_E), false);

        // Assert: the peer remains tracked as explicitly known-empty (not Unknown)
        assert!(
            matches!(
                mgr.observed_peer_subnets.get(&peer),
                Some(ObservedPeerSubnets::KnownEmpty)
            ),
            "Peer should transition to KnownEmpty after unsubscribing from all observed subnets"
        );
        assert!(
            aggregated_bitfield(&mgr, &peer).is_some(),
            "KnownEmpty peers should return an explicit zero bitfield"
        );
    }

    #[test]
    fn test_partial_unsubscribe_keeps_peer_entry() {
        // Arrange
        let mut mgr = create_test_manager();
        let peer = connect_random_peer(&mut mgr);
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_E), true);

        // Act: unsubscribe from Alan only
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), false);

        // Assert: peer entry still present (Boole fork still has a subscription)
        assert!(
            mgr.observed_peer_subnets.contains_key(&peer),
            "Peer entry must remain while at least one fork bitmap is non-empty"
        );
    }

    #[test]
    fn test_newly_connected_peer_is_unknown_and_returns_none_in_transition() {
        // Arrange: WarmUp state
        let mut mgr = create_test_manager_with_lifecycle(ForkLifecycle::WarmUp {
            current: alan_config(),
            upcoming: boole_config(),
        });
        let peer = connect_random_peer(&mut mgr);

        // Assert: Unknown state yields None (allows ENR fallback)
        assert!(
            aggregated_bitfield(&mgr, &peer).is_none(),
            "Newly connected peers should start in Unknown state"
        );
    }

    #[test]
    fn test_known_empty_blocks_enr_fallback_path() {
        // Arrange: WarmUp state with no peer ENR available in store
        let mut mgr = create_test_manager_with_lifecycle(ForkLifecycle::WarmUp {
            current: alan_config(),
            upcoming: boole_config(),
        });
        let peer = connect_random_peer(&mut mgr);
        let peer_store = MemoryStore::new(peer_store::memory_store::Config::default());
        let needed = HashSet::from([subnet(SUBNET_A)]);

        // Unknown -> no observed evidence yet, so fallback path is lenient
        assert!(
            mgr.peer_offers_needed_subnets_with_enr_fallback(&peer, &peer_store, &needed),
            "Unknown peers should be treated as no-observation-yet in fallback path"
        );

        // Act: observe explicit unsubscribe, transitioning to KnownEmpty
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), false);

        // Assert: KnownEmpty returns explicit zero bitfield and no fallback is applied
        assert!(
            !mgr.peer_offers_needed_subnets_with_enr_fallback(&peer, &peer_store, &needed),
            "KnownEmpty peers should not use ENR fallback"
        );
    }

    // ==================== count_observed_peers_for_subnets with multi-fork ====================

    #[test]
    fn test_count_observed_peers_counts_across_forks() {
        // Arrange: grace period aggregates across forks
        let mut mgr = create_grace_period_manager();
        let peer_a = connect_random_peer(&mut mgr);
        let peer_b = connect_random_peer(&mut mgr);

        // peer_a subscribes to SUBNET_C on Alan
        mgr.set_peer_subscribed(peer_a, Fork::Alan, subnet(SUBNET_C), true);
        // peer_b subscribes to SUBNET_C on Boole (different fork, same subnet)
        mgr.set_peer_subscribed(peer_b, Fork::Boole, subnet(SUBNET_C), true);
        // peer_a also subscribes to SUBNET_D on Boole
        mgr.set_peer_subscribed(peer_a, Fork::Boole, subnet(SUBNET_D), true);

        // Act
        let counts = mgr.count_observed_peers_for_subnets(&[
            subnet(SUBNET_C),
            subnet(SUBNET_D),
            subnet(SUBNET_UNSUBSCRIBED),
        ]);

        // Assert
        assert_eq!(
            counts,
            vec![2, 1, 0],
            "SUBNET_C should have 2 peers, SUBNET_D should have 1, SUBNET_UNSUBSCRIBED should have 0"
        );
    }

    #[test]
    fn test_count_observed_peers_requires_connected() {
        // Arrange: subscribe a peer but do NOT connect it via on_connection_established
        let mut mgr = create_test_manager();
        let disconnected_peer = PeerId::random();
        mgr.set_peer_subscribed(disconnected_peer, Fork::Alan, subnet(SUBNET_A), true);

        // Also add a properly connected peer for the same subnet
        let connected_peer = connect_random_peer(&mut mgr);
        mgr.set_peer_subscribed(connected_peer, Fork::Alan, subnet(SUBNET_A), true);

        // Act
        let counts = mgr.count_observed_peers_for_subnets(&[subnet(SUBNET_A)]);

        // Assert: only the connected peer should be counted
        assert_eq!(
            counts,
            vec![1],
            "Only connected peers should be counted; disconnected peer must be excluded"
        );
    }

    // ==================== peer_offers_needed_subnets_observed_only with multi-fork
    // ====================

    #[test]
    fn test_peer_offers_needed_subnets_across_forks() {
        // Arrange: grace period aggregates across forks
        let mut mgr = create_grace_period_manager();
        let peer = connect_random_peer(&mut mgr);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_D), true);

        let needed = HashSet::from([subnet(SUBNET_D)]);

        // Act & Assert
        assert!(
            mgr.peer_offers_needed_subnets_observed_only(&peer, &needed),
            "Peer subscribed on Boole should satisfy the needed set"
        );
    }

    #[test]
    fn test_peer_does_not_offer_unsubscribed_subnets() {
        // Arrange
        let mut mgr = create_test_manager();
        let peer = connect_random_peer(&mut mgr);
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_C), true);

        let needed = HashSet::from([subnet(SUBNET_E)]);

        // Act & Assert
        assert!(
            !mgr.peer_offers_needed_subnets_observed_only(&peer, &needed),
            "Peer subscribed to a different subnet should not satisfy the needed set"
        );
    }

    /// Verifies that after unsubscribing from one fork, the peer still offers
    /// the subnet via the remaining fork.
    #[test]
    fn test_peer_offers_needed_subnets_after_partial_unsubscribe() {
        // Arrange: grace period aggregates across forks
        let mut mgr = create_grace_period_manager();
        let peer = connect_random_peer(&mut mgr);
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_B), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_B), true);

        // Act: unsubscribe from Alan
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_B), false);

        let needed = HashSet::from([subnet(SUBNET_B)]);

        // Assert: Boole still provides the subnet
        assert!(
            mgr.peer_offers_needed_subnets_observed_only(&peer, &needed),
            "Peer should still offer subnet after unsubscribing only from Alan"
        );
    }

    // ==================== Idempotency ====================

    #[test]
    fn test_duplicate_subscribe_then_single_unsubscribe_clears_bit() {
        // Arrange
        let mut mgr = create_test_manager();
        let peer = connect_random_peer(&mut mgr);

        // Act: subscribe twice on the same fork+subnet, then unsubscribe once
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);
        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), false);

        // Assert: bit should be cleared — bitfield tracks presence, not a count
        assert!(
            !is_subnet_set(&mgr, &peer, SUBNET_A),
            "A single unsubscribe should clear the bit regardless of duplicate subscribes"
        );
    }

    // ==================== Fork-aware peer selection ====================

    #[test]
    fn test_normal_alan_only_shows_alan_bitmap() {
        // Arrange: Normal state on Alan
        let mut mgr = create_test_manager();
        let peer = connect_random_peer(&mut mgr);

        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_B), true);

        // Assert: only Alan bitmap is visible
        assert!(is_subnet_set(&mgr, &peer, SUBNET_A));
        assert!(
            !is_subnet_set(&mgr, &peer, SUBNET_B),
            "Boole subnet should not be visible in Normal(Alan) state"
        );
    }

    #[test]
    fn test_warmup_aggregates_both_forks() {
        // Arrange: WarmUp state (preparing for Boole)
        let mut mgr = create_test_manager_with_lifecycle(ForkLifecycle::WarmUp {
            current: alan_config(),
            upcoming: boole_config(),
        });
        let peer = connect_random_peer(&mut mgr);

        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_B), true);

        // Assert: both forks' bitmaps are aggregated
        assert!(is_subnet_set(&mgr, &peer, SUBNET_A));
        assert!(is_subnet_set(&mgr, &peer, SUBNET_B));
    }

    #[test]
    fn test_grace_period_aggregates_both_forks() {
        // Arrange: GracePeriod state
        let mut mgr = create_grace_period_manager();
        let peer = connect_random_peer(&mut mgr);

        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_B), true);

        // Assert: both forks' bitmaps are aggregated
        assert!(is_subnet_set(&mgr, &peer, SUBNET_A));
        assert!(is_subnet_set(&mgr, &peer, SUBNET_B));
    }

    #[test]
    fn test_normal_boole_only_shows_boole_bitmap() {
        // Arrange: Normal state on Boole (post-grace-period)
        let mut mgr = create_test_manager_with_lifecycle(ForkLifecycle::Normal {
            current: boole_config(),
        });
        let peer = connect_random_peer(&mut mgr);

        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);
        mgr.set_peer_subscribed(peer, Fork::Boole, subnet(SUBNET_B), true);

        // Assert: only Boole bitmap is visible
        assert!(
            !is_subnet_set(&mgr, &peer, SUBNET_A),
            "Alan subnet should not be visible in Normal(Boole) state"
        );
        assert!(is_subnet_set(&mgr, &peer, SUBNET_B));
    }

    #[test]
    fn test_peer_only_on_alan_becomes_invisible_after_boole_normal() {
        // Arrange: peer only subscribed to Alan subnets
        let mut mgr = create_test_manager_with_lifecycle(ForkLifecycle::Normal {
            current: boole_config(),
        });
        let peer = connect_random_peer(&mut mgr);

        mgr.set_peer_subscribed(peer, Fork::Alan, subnet(SUBNET_A), true);

        // Assert: observed state is known, but no current-fork bits are set
        let bf = aggregated_bitfield(&mgr, &peer)
            .expect("Peer with known old-fork subscriptions should return zero bitfield");
        assert!(
            !bf.get(SUBNET_A as usize).unwrap_or(false),
            "Alan-only subscriptions should not be visible in Normal(Boole) state"
        );

        // Also verify it doesn't offer needed subnets
        let needed = HashSet::from([subnet(SUBNET_A)]);
        assert!(
            !mgr.peer_offers_needed_subnets_observed_only(&peer, &needed),
            "Peer only on Alan should not offer subnets in Normal(Boole) state"
        );
    }
}
