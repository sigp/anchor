use libp2p::swarm::dial_opts::DialOpts;
use peer_store::memory_store;
use subnet_service::SubnetId;

/// Actions that the peer manager can request from the network
#[derive(Debug)]
pub struct ConnectActions {
    pub dial: Vec<DialOpts>,
    pub discover: Vec<SubnetId>,
}

impl ConnectActions {
    pub fn none() -> Self {
        ConnectActions {
            dial: vec![],
            discover: vec![],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.dial.is_empty() && self.discover.is_empty()
    }
}

/// Events emitted by the peer manager
#[derive(Debug)]
pub enum Event {
    PeerStore(peer_store::Event<memory_store::Event>),
    ConnectActions(ConnectActions),
}
