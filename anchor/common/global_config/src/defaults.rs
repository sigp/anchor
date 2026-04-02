//! Default values for global configuration flags.

/// Default network, used to partition the data storage
pub const DEFAULT_HARDCODED_NETWORK: &str = "mainnet";

pub struct NodeEndpoints {
    pub beacon_node: &'static str,
    pub execution_node: &'static str,
    pub execution_node_ws: &'static str,
}

/// Default node endpoints.
pub const DEFAULT_NODE_ENDPOINTS: NodeEndpoints = NodeEndpoints {
    beacon_node: "http://localhost:5052/",
    execution_node: "http://localhost:8545/",
    execution_node_ws: "ws://localhost:8546/",
};

pub struct MetricsDefaults {
    pub port: u16,
    pub port_str: &'static str, /* Used as an alternative to .parse().expect() risking runtime
                                 * panics. */
    pub host: &'static str,
}

/// Default metrics configuration.
pub const DEFAULT_METRICS: MetricsDefaults = MetricsDefaults {
    port: 5164,
    port_str: "5164",
    host: "127.0.0.1",
};

pub struct HttpApiDefaults {
    pub host: &'static str,
    pub port: u16,
    pub port_str: &'static str, /* Used as an alternative to .parse().expect() risking runtime
                                 * panics. */
}

/// Default HTTP API configuration.
pub const DEFAULT_HTTP_API: HttpApiDefaults = HttpApiDefaults {
    host: "127.0.0.1",
    port: 5062,
    port_str: "5062", // Used as an alternative to .parse().expect() risking runtime panics.
};

pub struct NetworkDefaults {
    pub address: &'static str,
}

/// Default network configuration.
pub const DEFAULT_NETWORK: NetworkDefaults = NetworkDefaults { address: "0.0.0.0" };

pub struct ExternalApiDefaults {
    pub sync_tolerances: &'static str,
}

/// Default external API configuration.
pub const DEFAULT_EXTERNAL_API: ExternalApiDefaults = ExternalApiDefaults {
    sync_tolerances: "8,8,48",
};
