//! Configuration for Anchor's HTTP API

use std::net::{IpAddr, Ipv4Addr};

use tower_http::cors::AllowOrigin;

/// Configuration for the HTTP server.
#[derive(Debug, Clone)]
pub struct Config {
    pub enabled: bool,
    pub listen_addr: IpAddr,
    pub listen_port: u16,
    pub allow_origin: AllowOrigin,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            listen_port: 5062,
            allow_origin: AllowOrigin::any(),
        }
    }
}
