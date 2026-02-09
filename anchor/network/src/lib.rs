#![allow(dead_code)]

mod behaviour;
mod config;
mod discovery;
mod handshake;
mod keypair_utils;
mod metrics;
mod network;
mod peer_manager;
mod scoring;
mod transport;

use std::sync::Arc;

pub use config::{Config, DEFAULT_DISC_PORT, DEFAULT_QUIC_PORT, DEFAULT_TCP_PORT};
pub use network::Network;
pub use network_utils::listen_addr::{ListenAddr, ListenAddress};
use parking_lot::RwLock;
use ssv_types::domain_type::DomainType;
pub type Enr = discv5::enr::Enr<discv5::enr::CombinedKey>;
pub use peer_manager::types::{ClientType, PeerInfo};

/// A shared, thread-safe domain type that serves as a single source of truth.
///
/// # Lifecycle
///
/// Created by [`Network::try_new`] with the initial domain type from config. Clones are
/// passed to `Discovery` and `handshake::Behaviour` so all three components share the
/// same underlying value.
///
/// # Updates
///
/// When a fork activates, [`Network::on_fork_phase`] calls [`SharedDomainType::set`] once.
/// All components reading via [`SharedDomainType::get`] see the new domain type immediately,
/// eliminating the need for per-component update methods.
///
/// # Concurrency
///
/// Uses [`parking_lot::RwLock`] for interior mutability. Writes are brief (single `Copy`
/// assignment) and rare (only on fork activation), so contention is negligible.
#[derive(Clone, Debug)]
pub(crate) struct SharedDomainType(Arc<RwLock<DomainType>>);

impl SharedDomainType {
    pub fn new(domain_type: DomainType) -> Self {
        Self(Arc::new(RwLock::new(domain_type)))
    }

    pub fn get(&self) -> DomainType {
        *self.0.read()
    }

    pub fn set(&self, domain_type: DomainType) {
        *self.0.write() = domain_type;
    }
}
