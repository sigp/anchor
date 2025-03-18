use crate::discovery::Discovery;
use crate::handshake;
use crate::peer_manager::PeerManager;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{identify, ping};

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
    /// Anchor peer manager, wrapping libp2p behaviours with minimal added logic for peer selection.
    pub peer_manager: PeerManager,

    pub handshake: handshake::Behaviour,
}
