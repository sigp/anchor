#![allow(dead_code)]

mod behaviour;
mod config;
mod discovery;
mod handshake;
mod keypair_utils;
mod network;
mod peer_manager;
mod transport;
pub use config::{
    Config, DEFAULT_DISC_PORT, DEFAULT_IPV4_ADDRESS, DEFAULT_QUIC_PORT, DEFAULT_TCP_PORT,
};
pub use lighthouse_network::{ListenAddr, ListenAddress};
pub use network::Network;
pub use discovery::load_enr_from_disk;

pub type Enr = discv5::enr::Enr<discv5::enr::CombinedKey>;

pub const SUBNET_COUNT: usize = 128;
type SubnetBits = [u8; SUBNET_COUNT / 8];
