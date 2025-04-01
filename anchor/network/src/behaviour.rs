use libp2p::{identify, ping, swarm::NetworkBehaviour};

use crate::{discovery::Discovery, handshake, peer_manager::PeerManager};

#[derive(NetworkBehaviour)]
pub struct AnchorBehaviour {
    /// Provides IP addresses and peer information.
    pub identify: identify::Behaviour,
    /// Used for connection health checks.
    pub ping: ping::Behaviour,
    /// The routing pub-sub mechanism for Anchor.
    pub gossipsub: gossipsub::Behaviour,
    /// Discv5 Discovery protocol.
    pub discovery: Discovery,
    /// Anchor peer manager, wrapping libp2p behaviours with minimal added logic for peer
    /// selection.
    pub peer_manager: PeerManager,

    pub handshake: handshake::Behaviour,
}
