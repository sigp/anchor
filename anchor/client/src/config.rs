// use crate::{http_api, http_metrics};
// use clap_utils::{flags::DISABLE_MALLOC_TUNING_FLAG, parse_optional, parse_required};

use std::{net::IpAddr, path::PathBuf};

use global_config::GlobalConfig;
use multiaddr::{Multiaddr, Protocol};
use network::{DEFAULT_DISC_PORT, DEFAULT_TCP_PORT, ListenAddr, ListenAddress};
use network_utils::unused_port::{
    unused_tcp4_port, unused_tcp6_port, unused_udp4_port, unused_udp6_port,
};
use sensitive_url::SensitiveUrl;
use ssv_types::OperatorId;
use tracing::{error, warn};

use crate::cli::Node;

pub const DEFAULT_BEACON_NODE: &str = "http://localhost:5052/";
pub const DEFAULT_EXECUTION_NODE: &str = "http://localhost:8545/";
pub const DEFAULT_EXECUTION_NODE_WS: &str = "ws://localhost:8546/";

/// Stores the core configuration for this Anchor instance.
#[derive(Clone)]
pub struct Config {
    /// The global config, containing datadir and SSV network to connect to.
    pub global_config: GlobalConfig,
    /// Path to the key file to use
    pub key_file: Option<PathBuf>,
    /// Path to a password file to use
    pub password_file: Option<PathBuf>,
    /// The http endpoints of the beacon node APIs.
    ///
    /// Should be similar to `["http://localhost:8080"]`
    pub beacon_nodes: Vec<SensitiveUrl>,
    /// An optional beacon node used for block proposals only.
    pub proposer_nodes: Vec<SensitiveUrl>,
    /// The http endpoints of the execution node APIs.
    pub execution_nodes: Vec<SensitiveUrl>,
    /// The websocket endpoints of the execution node APIs.
    pub execution_nodes_websocket: SensitiveUrl,
    /// beacon node is not synced at startup.
    pub allow_unsynced_beacon_node: bool,
    /// If true, use longer timeouts for requests made to the beacon node.
    pub use_long_timeouts: bool,
    /// Configuration for the HTTP REST API.
    pub http_api: http_api::Config,
    /// Configuration for the network stack.
    pub network: network::Config,
    /// Configuration for the HTTP REST API.
    pub http_metrics: http_metrics::Config,
    /// Should we gather per validator metrics for > 64 validators.
    pub enable_high_validator_count_metrics: bool,
    /// A list of custom certificates that the validator client will additionally use when
    /// connecting to a beacon node over SSL/TLS.
    pub beacon_nodes_tls_certs: Option<Vec<PathBuf>>,
    /// A list of custom certificates that the validator client will additionally use when
    /// connecting to an execution node over SSL/TLS.
    pub execution_nodes_tls_certs: Option<Vec<PathBuf>>,
    /// Configuration for the processor
    pub processor: processor::Config,
    /// If slashing protection is disabled
    pub disable_slashing_protection: bool,
    /// Act as impostor
    pub impostor: Option<OperatorId>,
    /// Gas limit on blocks
    pub gas_limit: u64,
    /// Should payload construction be outsourced
    pub builder_proposals: bool,
    /// Block boost factor
    pub builder_boost_factor: Option<u64>,
    /// Should external payloads always be preferred
    pub prefer_builder_proposals: bool,
    /// Controls whether the latency measurement service is enabled
    pub disable_latency_measurement_service: bool,
    /// Enable operator doppelgänger protection (blocks messages and monitors for twins)
    pub operator_dg: bool,
    /// Number of epochs to monitor for twins after grace period
    pub operator_dg_wait_epochs: u64,
    /// Whether to check for matching checkpoint roots in QBFT.
    pub strict_mfp: bool,
}

impl Config {
    /// Build a new configuration from defaults.
    ///
    /// global_config: We pass this because it would be expensive to uselessly get a default.
    fn new(global_config: GlobalConfig) -> Self {
        let beacon_nodes = vec![
            SensitiveUrl::parse(DEFAULT_BEACON_NODE)
                .expect("beacon_nodes must always be a valid url."),
        ];
        let execution_nodes = vec![
            SensitiveUrl::parse(DEFAULT_EXECUTION_NODE)
                .expect("execution_nodes must always be a valid url."),
        ];
        let execution_nodes_websocket = SensitiveUrl::parse(DEFAULT_EXECUTION_NODE_WS)
            .expect("execution_nodes_websocket must always be a valid url.");
        let network_config = network::Config::new(global_config.data_dir.network_dir());

        Self {
            global_config,
            key_file: None,
            password_file: None,
            beacon_nodes,
            proposer_nodes: vec![],
            execution_nodes,
            execution_nodes_websocket,
            allow_unsynced_beacon_node: false,
            use_long_timeouts: false,
            http_api: <_>::default(),
            http_metrics: <_>::default(),
            enable_high_validator_count_metrics: false,
            network: network_config,
            beacon_nodes_tls_certs: None,
            execution_nodes_tls_certs: None,
            processor: <_>::default(),
            disable_slashing_protection: false,
            impostor: None,
            builder_proposals: false,
            builder_boost_factor: None,
            prefer_builder_proposals: false,
            gas_limit: 36_000_000,
            disable_latency_measurement_service: false,
            operator_dg: false,
            operator_dg_wait_epochs: 2,
            strict_mfp: false,
        }
    }
}

/// Returns a `Default` implementation of `Self` with some parameters modified by the supplied
/// `cli_args`.
pub fn from_cli(cli_args: &Node, global_config: GlobalConfig) -> Result<Config, String> {
    let mut config = Config::new(global_config);

    config.key_file = cli_args.key_file.clone();
    config.password_file = cli_args.password_file.clone();

    if let Some(ref beacon_nodes) = cli_args.beacon_nodes {
        parse_urls(&mut config.beacon_nodes, beacon_nodes, "beacon node")?;
    }
    if let Some(ref execution_rpc) = cli_args.execution_rpc {
        parse_urls(&mut config.execution_nodes, execution_rpc, "execution RPC")?;
    }
    if let Some(ref execution_ws) = cli_args.execution_ws {
        let ws =
            SensitiveUrl::parse(execution_ws).map_err(|e| format!("Unable to parse URL: {e:?}"))?;
        config.execution_nodes_websocket = ws;
    }

    // Status of slashing protection
    config.disable_slashing_protection = cli_args.disable_slashing_protection;

    // Network related
    config.network.listen_addresses = parse_listening_addresses(cli_args)?;

    for addr in cli_args.boot_nodes.clone() {
        match addr.parse() {
            Ok(enr) => config.network.boot_nodes_enr.push(enr),
            Err(_) => {
                // parsing as ENR failed, try as Multiaddr
                let multi: Multiaddr = addr
                    .parse()
                    .map_err(|_| format!("Not valid as ENR nor Multiaddr: {addr}"))?;
                if !multi.iter().any(|proto| matches!(proto, Protocol::Udp(_))) {
                    error!(addr = multi.to_string(), "Missing UDP in Multiaddr");
                }
                if !multi.iter().any(|proto| matches!(proto, Protocol::P2p(_))) {
                    error!(addr = multi.to_string(), "Missing P2P in Multiaddr");
                }
                config.network.boot_nodes_multiaddr.push(multi);
            }
        }
    }
    if cli_args.boot_nodes.is_empty() {
        config.network.boot_nodes_enr = config
            .global_config
            .ssv_network
            .ssv_boot_nodes
            .clone()
            .unwrap_or_default();
    }

    config.network.enr_address = (cli_args.enr_address, cli_args.enr_address6);
    config.network.disable_enr_auto_update = cli_args.disable_enr_auto_update;
    config.network.enr_tcp4_port = cli_args.enr_tcp_port;
    config.network.enr_udp4_port = cli_args.enr_udp_port;
    config.network.enr_quic4_port = cli_args.enr_quic_port;
    config.network.enr_tcp6_port = cli_args.enr_tcp6_port;
    config.network.enr_udp6_port = cli_args.enr_udp6_port;
    config.network.enr_quic6_port = cli_args.enr_quic6_port;

    config.network.subscribe_all_subnets = cli_args.subscribe_all_subnets;

    config.network.target_peers = cli_args.target_peers;

    // Network related - set peer scoring configuration
    config.network.disable_gossipsub_peer_scoring = cli_args.disable_gossipsub_peer_scoring;
    config.network.disable_gossipsub_topic_scoring = cli_args.disable_gossipsub_topic_scoring;

    config.beacon_nodes_tls_certs = cli_args.beacon_nodes_tls_certs.clone();
    config.execution_nodes_tls_certs = cli_args.execution_nodes_tls_certs.clone();

    // MEV options
    config.builder_proposals = cli_args.builder_proposals;
    config.builder_boost_factor = cli_args.builder_boost_factor;
    config.prefer_builder_proposals = cli_args.prefer_builder_proposals;

    config.gas_limit = cli_args.gas_limit;

    // Http API server
    config.http_api.enabled = cli_args.http;

    if let Some(address) = cli_args.http_address {
        if cli_args.unencrypted_http_transport {
            config.http_api.listen_addr = address;
        } else {
            return Err(
                "While using `--http-address`, you must also use `--unencrypted-http-transport`."
                    .to_string(),
            );
        }
    }

    if let Some(port) = cli_args.http_port {
        config.http_api.listen_port = port;
    }

    if let Some(allow_origin) = &cli_args.http_allow_origin {
        // Pre-validate the config value to give feedback to the user on node startup, instead of
        // as late as when the first API response is produced.
        hyper::header::HeaderValue::from_str(allow_origin)
            .map_err(|_| "Invalid allow-origin value")?;

        config.http_api.allow_origin = Some(allow_origin.to_string());
    }

    // Prometheus metrics HTTP server

    if cli_args.metrics {
        config.http_metrics.enabled = true;
    }

    if let Some(address) = cli_args.metrics_address {
        config.http_metrics.listen_addr = address;
    }

    if let Some(port) = cli_args.metrics_port {
        config.http_metrics.listen_port = port;
    }

    config.enable_high_validator_count_metrics = cli_args.enable_high_validator_count_metrics;

    config.impostor = cli_args.impostor.map(OperatorId);
    config.disable_latency_measurement_service = cli_args.disable_latency_measurement_service;

    // Operator doppelgänger protection
    config.operator_dg = cli_args.operator_dg;
    config.operator_dg_wait_epochs = cli_args.operator_dg_wait_epochs;

    // Majority fork protection
    config.strict_mfp = cli_args.strict_mfp;

    // Performance options
    if let Some(max_workers) = cli_args.max_workers {
        config.processor.max_workers = max_workers;
    };

    for size_spec in &cli_args.work_queue_size {
        let Some((queue, size)) = size_spec.split_once('=') else {
            return Err(format!("Invalid queue size specification: {size_spec}"));
        };
        let Ok(queue) = queue.trim().parse() else {
            return Err(format!("Unknown queue: {size}"));
        };
        let Ok(size) = size.trim().parse() else {
            return Err(format!("Not a number: {size}"));
        };
        config.processor.queue_size.insert(queue, size);
    }

    Ok(config)
}

/// Read SensitiveUrls from given CLI Strings
fn parse_urls(dest: &mut Vec<SensitiveUrl>, src: &[String], kind: &str) -> Result<(), String> {
    *dest = src
        .iter()
        .map(|s| SensitiveUrl::parse(s))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Unable to parse {kind} URL: {e:?}"))?;
    Ok(())
}

/// Gets the listening_addresses for lighthouse based on the cli options.
pub fn parse_listening_addresses(cli_args: &Node) -> Result<ListenAddress, String> {
    // parse the possible ips
    let mut maybe_ipv4 = None;
    let mut maybe_ipv6 = None;
    for addr in cli_args.listen_addresses.iter() {
        match addr {
            IpAddr::V4(v4_addr) => match &maybe_ipv4 {
                Some(first_ipv4_addr) => {
                    return Err(format!(
                        "When setting the --listen-address option twice, use an IpV4 address and an Ipv6 address. \
                                Got two IpV4 addresses {first_ipv4_addr} and {v4_addr}"
                    ));
                }
                None => maybe_ipv4 = Some(v4_addr),
            },
            IpAddr::V6(v6_addr) => match &maybe_ipv6 {
                Some(first_ipv6_addr) => {
                    return Err(format!(
                        "When setting the --listen-address option twice, use an IpV4 address and an Ipv6 address. \
                                Got two IpV6 addresses {first_ipv6_addr} and {v6_addr}"
                    ));
                }
                None => maybe_ipv6 = Some(v6_addr),
            },
        }
    }

    // Now put everything together
    let listening_addresses = match (maybe_ipv4, maybe_ipv6) {
        (None, None) => {
            // This should never happen unless clap is broken
            return Err("No listening addresses provided".into());
        }
        (None, Some(ipv6)) => {
            // A single ipv6 address was provided. Set the ports
            if cli_args.port6.is_some() {
                warn!(
                    "When listening only over IPv6, use the --port flag. The value of --port6 will be ignored."
                );
            }

            if cli_args.discovery_port6.is_some() {
                warn!(
                    "When listening only over IPv6, use the --discovery-port flag. The value of --discovery-port6 will be ignored."
                )
            }

            if cli_args.quic_port6.is_some() {
                warn!(
                    "When listening only over IPv6, use the --quic-port flag. The value of --quic-port6 will be ignored."
                )
            }

            // Select the QUIC port in the following order of precedence:
            // 1. If use_zero_ports is set, use an unused TCP6 port.
            // 2. Else, if port is specified, use it.
            // 3. If none of the above are set, use the default TCP port (DEFAULT_TCP_PORT).
            let tcp_port = cli_args
                .use_zero_ports
                .then(unused_tcp6_port)
                .transpose()?
                .or(cli_args.port)
                .unwrap_or(DEFAULT_TCP_PORT);

            // Select the discovery port in the following order of precedence:
            // 1. If use_zero_ports is set, use an unused UDP6 port.
            // 2. Else, if discovery_port is specified in CLI args, use it.
            // 3. Else, if port is specified, use it as the fallback.
            // 4. If none of the above are set, use the default discovery port (DEFAULT_DISC_PORT).
            let disc_port = cli_args
                .use_zero_ports
                .then(unused_udp6_port)
                .transpose()?
                .or(cli_args.discovery_port)
                .or(cli_args.port)
                .unwrap_or(DEFAULT_DISC_PORT);

            // Select the QUIC port in the following order of precedence:
            // 1. If use_zero_ports is set, use an unused UDP6 port.
            // 2. Else, if quic_port is specified, use it.
            // 3. If none of the above are set, use the selected TCP port + 1.
            let quic_port = cli_args
                .use_zero_ports
                .then(unused_udp6_port)
                .transpose()?
                .or(cli_args.quic_port)
                .unwrap_or(if tcp_port == 0 { 0 } else { tcp_port + 1 });

            ListenAddress::V6(ListenAddr {
                addr: *ipv6,
                quic_port,
                disc_port,
                tcp_port,
            })
        }
        (Some(ipv4), None) => {
            // A single ipv4 address was provided. Set the ports

            // Select the TCP port in the following order of precedence:
            // 1. If use_zero_ports is set, use an unused TCP4 port.
            // 2. Else, if port is specified, use it.
            // 3. If none of the above are set, use the default TCP port (DEFAULT_TCP_PORT).
            let tcp_port = cli_args
                .use_zero_ports
                .then(unused_tcp4_port)
                .transpose()?
                .or(cli_args.port)
                .unwrap_or(DEFAULT_TCP_PORT);
            // Select the discovery port in the following order of precedence:
            // 1. If use_zero_ports is set, use an unused UDP4 port.
            // 2. Else, if discovery_port is specified in CLI args, use it.
            // 3. Else, if port is specified, use it as the fallback.
            // 4. If none of the above are set, use the default discovery port (DEFAULT_DISC_PORT).
            let disc_port = cli_args
                .use_zero_ports
                .then(unused_udp4_port)
                .transpose()?
                .or(cli_args.discovery_port)
                .or(cli_args.port)
                .unwrap_or(DEFAULT_DISC_PORT);
            // Select the QUIC port in the following order of precedence:
            // 1. If use_zero_ports is set, use an unused UDP4 port.
            // 2. Else, if quic_port is specified, use it.
            // 3. If none of the above are set, use the selected TCP port + 1.
            let quic_port = cli_args
                .use_zero_ports
                .then(unused_udp4_port)
                .transpose()?
                .or(cli_args.quic_port)
                .unwrap_or(if tcp_port == 0 { 0 } else { tcp_port + 1 });

            ListenAddress::V4(ListenAddr {
                addr: *ipv4,
                disc_port,
                quic_port,
                tcp_port,
            })
        }
        (Some(ipv4), Some(ipv6)) => {
            let ipv4_tcp_port = cli_args
                .use_zero_ports
                .then(unused_tcp4_port)
                .transpose()?
                .or(cli_args.port)
                .unwrap_or(DEFAULT_TCP_PORT);
            let ipv4_disc_port = cli_args
                .use_zero_ports
                .then(unused_udp4_port)
                .transpose()?
                .or(cli_args.discovery_port)
                .or(cli_args.port)
                .unwrap_or(DEFAULT_DISC_PORT);
            let ipv4_quic_port = cli_args
                .use_zero_ports
                .then(unused_udp4_port)
                .transpose()?
                .or(cli_args.quic_port)
                .unwrap_or(if ipv4_tcp_port == 0 {
                    0
                } else {
                    ipv4_tcp_port + 1
                });

            let ipv6_tcp_port = cli_args
                .use_zero_ports
                .then(unused_tcp6_port)
                .transpose()?
                .or(cli_args.port6)
                .unwrap_or(ipv4_tcp_port);
            let ipv6_disc_port = cli_args
                .use_zero_ports
                .then(unused_udp6_port)
                .transpose()?
                .or(cli_args.discovery_port6)
                .unwrap_or(ipv4_disc_port);
            let ipv6_quic_port = cli_args
                .use_zero_ports
                .then(unused_udp6_port)
                .transpose()?
                .or(cli_args.quic_port6)
                .unwrap_or(if ipv6_tcp_port == 0 {
                    0
                } else {
                    ipv6_tcp_port + 1
                });

            ListenAddress::DualStack(
                ListenAddr {
                    addr: *ipv4,
                    disc_port: ipv4_disc_port,
                    quic_port: ipv4_quic_port,
                    tcp_port: ipv4_tcp_port,
                },
                ListenAddr {
                    addr: *ipv6,
                    disc_port: ipv6_disc_port,
                    quic_port: ipv6_quic_port,
                    tcp_port: ipv6_tcp_port,
                },
            )
        }
    };

    Ok(listening_addresses)
}
