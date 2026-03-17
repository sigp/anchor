//! Configuration for Anchor's HTTP API

use std::net::{IpAddr, Ipv4Addr};

use tower_http::cors::AllowOrigin;

/// Configuration for the HTTP server.
#[derive(Debug, Clone)]
pub struct Config {
    pub enabled: bool,
    pub listen_addr: IpAddr,
    pub listen_port: u16,
    pub allow_origin: Option<AllowOrigin>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            listen_port: 5062,
            allow_origin: None,
        }
    }
}

impl Config {
    /// Returns the configured `AllowOrigin`, or falls back to the listen address and port.
    pub fn allow_origin(&self) -> AllowOrigin {
        self.allow_origin.clone().unwrap_or_else(|| {
            AllowOrigin::exact(
                format!("http://{}:{}", self.listen_addr, self.listen_port)
                    .parse()
                    .expect("listen address and port should produce a valid header value"),
            )
        })
    }
}
