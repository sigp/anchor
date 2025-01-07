#![allow(dead_code)]

mod behaviour;
mod config;
mod discovery;
mod keypair_utils;
mod network;
mod transport;
mod types;

pub use config::Config;
pub use lighthouse_network::{ListenAddr, ListenAddress};
pub use network::Network;

pub type Enr = discv5::enr::Enr<discv5::enr::CombinedKey>;
