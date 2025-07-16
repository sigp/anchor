use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use discv5::libp2p_identity::PeerId;
use libp2p::{
    Multiaddr, allow_block_list,
    core::{Endpoint, transport::PortUse},
    swarm::{ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour},
};
use tracing::debug;

use crate::scoring::peer_score_config::RETAIN_SCORE_EPOCH_MULTIPLIER;

/// Manages peer blocking functionality
pub struct BlockingManager {
    /// Block list behaviour for actual connection denial
    block_list: allow_block_list::Behaviour<allow_block_list::BlockedPeers>,
    /// Tracking when peers were blocked for automatic unblocking
    blocked_peers_timestamps: HashMap<PeerId, Instant>,
    /// One epoch duration for calculating retain_score timeout
    one_epoch_duration: Duration,
}

impl BlockingManager {
    pub fn new(one_epoch_duration: Duration) -> Self {
        Self {
            block_list: allow_block_list::Behaviour::<allow_block_list::BlockedPeers>::default(),
            blocked_peers_timestamps: HashMap::new(),
            one_epoch_duration,
        }
    }

    /// Block a peer based on poor gossipsub score
    pub fn block_peer_for_poor_score(&mut self, peer_id: PeerId) {
        if self.block_list.block_peer(peer_id) {
            self.blocked_peers_timestamps
                .insert(peer_id, Instant::now());
            debug!(?peer_id, "Blocked peer due to poor gossipsub score");
        }
    }

    /// Block a peer (generic method for use by Network)
    pub fn block_peer(&mut self, peer_id: PeerId) -> bool {
        self.block_list.block_peer(peer_id)
    }

    /// Unblock a peer and remove from tracking
    pub fn unblock_peer(&mut self, peer_id: PeerId) -> bool {
        let was_removed = self.block_list.unblock_peer(peer_id);
        if was_removed {
            self.blocked_peers_timestamps.remove(&peer_id);
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

        let peers_to_unblock: Vec<PeerId> = self
            .blocked_peers_timestamps
            .iter()
            .filter_map(|(&peer_id, &blocked_at)| {
                if blocked_at.elapsed() >= retain_score_duration {
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

    pub fn blocked_peers_count(&self) -> usize {
        self.blocked_peers_timestamps.len()
    }

    // Delegation methods for connection handling
    pub fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.block_list
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
    }

    pub fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.block_list
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr)
            .map(|_| ()) // Discard the handler, we just want to know if connection is allowed
    }

    pub fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        self.block_list.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )
    }

    pub fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<(), ConnectionDenied> {
        self.block_list
            .handle_established_outbound_connection(
                connection_id,
                peer,
                addr,
                role_override,
                port_use,
            )
            .map(|_| ()) // Discard the handler, we just want to know if connection is allowed
    }

    pub fn on_swarm_event(&mut self, event: FromSwarm) {
        self.block_list.on_swarm_event(event);
    }

    pub fn poll(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<libp2p::swarm::ToSwarm<std::convert::Infallible, ()>> {
        // Block list may have events, but we don't need to forward them
        let _ = self.block_list.poll(cx);
        std::task::Poll::Pending
    }
}
