#![allow(dead_code)]

mod behaviour;
mod config;
mod discovery;
mod handshake;
mod keypair_utils;
mod network;
mod peer_manager;
mod scoring;
mod transport;

pub use config::Config;
pub use lighthouse_network::{ListenAddr, ListenAddress};
pub use network::Network;
pub type Enr = discv5::enr::Enr<discv5::enr::CombinedKey>;
