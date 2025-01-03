use discv5::Enr;
use libp2p::Multiaddr;
use lighthouse_network::types::GossipKind;
use lighthouse_network::{ListenAddr, ListenAddress};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::num::NonZeroU16;
use std::path::PathBuf;

/// This is a default network directory, but it will be overridden by the cli defaults.
const DEFAULT_NETWORK_DIR: &str = ".anchor/network";

pub const DEFAULT_IPV4_ADDRESS: Ipv4Addr = Ipv4Addr::UNSPECIFIED;
pub const DEFAULT_TCP_PORT: u16 = 9100u16;
pub const DEFAULT_DISC_PORT: u16 = 9100u16;
pub const DEFAULT_QUIC_PORT: u16 = 9101u16;

/// Configuration for setting up the p2p network.
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    /// Data directory where node's keyfile is stored
    pub network_dir: PathBuf,

    /// IP addresses to listen on.
    pub listen_addresses: ListenAddress,

    /// The address to broadcast to peers about which address we are listening on. None indicates
    /// that no discovery address has been set in the CLI args.
    pub enr_address: (Option<Ipv4Addr>, Option<Ipv6Addr>),

    /// The udp ipv4 port to broadcast to peers in order to reach back for discovery.
    pub enr_udp4_port: Option<NonZeroU16>,

    /// The quic ipv4 port to broadcast to peers in order to reach back for libp2p services.
    pub enr_quic4_port: Option<NonZeroU16>,

    /// The tcp ipv4 port to broadcast to peers in order to reach back for libp2p services.
    pub enr_tcp4_port: Option<NonZeroU16>,

    /// The udp ipv6 port to broadcast to peers in order to reach back for discovery.
    pub enr_udp6_port: Option<NonZeroU16>,

    /// The tcp ipv6 port to broadcast to peers in order to reach back for libp2p services.
    pub enr_tcp6_port: Option<NonZeroU16>,

    /// The quic ipv6 port to broadcast to peers in order to reach back for libp2p services.
    pub enr_quic6_port: Option<NonZeroU16>,

    /// List of nodes to initially connect to.
    pub boot_nodes_enr: Vec<Enr>,

    /// List of nodes to initially connect to, on Multiaddr format.
    pub boot_nodes_multiaddr: Vec<Multiaddr>,

    /// Disables peer scoring altogether.
    pub disable_peer_scoring: bool,

    /// Disables the discovery protocol from starting.
    pub disable_discovery: bool,

    /// Disables quic support.
    pub disable_quic_support: bool,

    /// List of extra topics to initially subscribe to as strings.
    pub topics: Vec<GossipKind>,

    /// Target number of connected peers.
    pub target_peers: usize,
}

impl Default for Config {
    fn default() -> Self {
        // WARNING: this directory default should be always overwritten with parameters
        // from cli for specific networks.
        let network_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(DEFAULT_NETWORK_DIR);

        let listen_addresses = ListenAddress::V4(ListenAddr {
            addr: DEFAULT_IPV4_ADDRESS,
            disc_port: DEFAULT_DISC_PORT,
            quic_port: DEFAULT_QUIC_PORT,
            tcp_port: DEFAULT_TCP_PORT,
        });

        Self {
            network_dir,
            listen_addresses,
            enr_address: (None, None),
            enr_udp4_port: None,
            enr_quic4_port: None,
            enr_tcp4_port: None,
            enr_udp6_port: None,
            enr_tcp6_port: None,
            enr_quic6_port: None,
            target_peers: 50,
            boot_nodes_enr: vec![],
            boot_nodes_multiaddr: vec![],
            disable_peer_scoring: false,
            disable_discovery: false,
            disable_quic_support: false,
            topics: vec![],
        }
    }
}
