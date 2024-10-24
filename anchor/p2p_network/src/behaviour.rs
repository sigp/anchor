use libp2p::swarm::NetworkBehaviour;
use libp2p::{identify, ping};

#[derive(NetworkBehaviour)]
pub(crate) struct AnchorBehaviour {
    /// Provides IP addresses and peer information.
    pub identify: identify::Behaviour,
    /// Used for connection health checks.
    pub ping: ping::Behaviour,
    // /// The routing pub-sub mechanism for Anchor.
    // pub gossipsub: gossipsub::Behaviour,
}
