#![allow(dead_code)]

mod behaviour;
mod config;
mod discovery;
mod handshake;
mod keypair_utils;
mod network;
mod peer_manager;
mod peer_score_config;
mod transport;

pub use config::Config;
pub use lighthouse_network::{ListenAddr, ListenAddress};
pub use network::Network;
pub use peer_score_config::{peer_score_params, peer_score_thresholds};

pub type Enr = discv5::enr::Enr<discv5::enr::CombinedKey>;

pub const SUBNET_COUNT: usize = 128;
type SubnetBits = [u8; SUBNET_COUNT / 8];
