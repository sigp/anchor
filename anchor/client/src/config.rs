// use crate::{http_api, http_metrics};
// use clap_utils::{flags::DISABLE_MALLOC_TUNING_FLAG, parse_optional, parse_required};

use crate::cli::Anchor;
use network::{ListenAddr, ListenAddress};
use sensitive_url::SensitiveUrl;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use tracing::warn;

pub const DEFAULT_BEACON_NODE: &str = "http://localhost:5052/";
pub const DEFAULT_EXECUTION_NODE: &str = "http://localhost:8545/";
/// The default Data directory, relative to the users home directory
pub const DEFAULT_ROOT_DIR: &str = ".anchor";
/// Default network, used to partition the data storage
pub const DEFAULT_HARDCODED_NETWORK: &str = "mainnet";
/// Directory within the network directory where secrets are stored.
pub const DEFAULT_SECRETS_DIR: &str = "secrets";

/// Stores the core configuration for this Anchor instance.
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    /// The data directory, which stores all validator databases
    pub data_dir: PathBuf,
    /// The directory containing the passwords to unlock validator keystores.
    pub secrets_dir: PathBuf,
    /// The http endpoints of the beacon node APIs.
    ///
    /// Should be similar to `["http://localhost:8080"]`
    pub beacon_nodes: Vec<SensitiveUrl>,
    /// The http endpoints of the execution node APIs.
    pub execution_nodes: Vec<SensitiveUrl>,
    /// beacon node is not synced at startup.
    pub allow_unsynced_beacon_node: bool,
    /// Configuration for the HTTP REST API.
    pub http_api: http_api::Config,
    /// Configuration for the network stack.
    pub network: network::Config,
    /// Configuration for the HTTP REST API.
    pub http_metrics: http_metrics::Config,
    /// A list of custom certificates that the validator client will additionally use when
    /// connecting to a beacon node over SSL/TLS.
    pub beacon_nodes_tls_certs: Option<Vec<PathBuf>>,
    /// A list of custom certificates that the validator client will additionally use when
    /// connecting to an execution node over SSL/TLS.
    pub execution_nodes_tls_certs: Option<Vec<PathBuf>>,
    /// Configuration for the processor
    pub processor: processor::Config,
}

impl Default for Config {
    /// Build a new configuration from defaults.
    fn default() -> Self {
        // WARNING: these directory defaults should be always overwritten with parameters from cli
        // for specific networks.
        let data_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(DEFAULT_ROOT_DIR)
            .join(DEFAULT_HARDCODED_NETWORK);
        let secrets_dir = data_dir.join(DEFAULT_SECRETS_DIR);

        let beacon_nodes = vec![SensitiveUrl::parse(DEFAULT_BEACON_NODE)
            .expect("beacon_nodes must always be a valid url.")];
        let execution_nodes = vec![SensitiveUrl::parse(DEFAULT_EXECUTION_NODE)
            .expect("execution_nodes must always be a valid url.")];
        Self {
            data_dir,
            secrets_dir,
            beacon_nodes,
            execution_nodes,
            allow_unsynced_beacon_node: false,
            http_api: <_>::default(),
            http_metrics: <_>::default(),
            network: <_>::default(),
            beacon_nodes_tls_certs: None,
            execution_nodes_tls_certs: None,
            processor: <_>::default(),
        }
    }
}

/// Returns a `Default` implementation of `Self` with some parameters modified by the supplied
/// `cli_args`.
pub fn from_cli(cli_args: &Anchor) -> Result<Config, String> {
    let mut config = Config::default();

    let default_root_dir = dirs::home_dir()
        .map(|home| home.join(DEFAULT_ROOT_DIR))
        .unwrap_or_else(|| PathBuf::from("."));

    let (mut data_dir, mut secrets_dir) = (None, None);

    if let Some(datadir) = cli_args.datadir.clone() {
        secrets_dir = Some(datadir.join(DEFAULT_SECRETS_DIR));
        data_dir = Some(datadir);
    }

    if cli_args.secrets_dir.is_some() {
        secrets_dir = cli_args.secrets_dir.clone();
    }

    config.data_dir = data_dir.unwrap_or_else(|| default_root_dir.join(DEFAULT_ROOT_DIR));

    config.secrets_dir = secrets_dir.unwrap_or_else(|| default_root_dir.join(DEFAULT_SECRETS_DIR));

    if !config.data_dir.exists() {
        fs::create_dir_all(&config.data_dir)
            .map_err(|e| format!("Failed to create {:?}: {:?}", config.data_dir, e))?;
    }

    if let Some(beacon_nodes) = &cli_args.beacon_nodes {
        config.beacon_nodes = beacon_nodes
            .iter()
            .map(|s| SensitiveUrl::parse(s))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("Unable to parse beacon node URL: {:?}", e))?;
    }

    if let Some(execution_nodes) = &cli_args.execution_nodes {
        config.execution_nodes = execution_nodes
            .iter()
            .map(|s| SensitiveUrl::parse(s))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("Unable to parse execution node URL: {:?}", e))?;
    }

    /*
     * Network related
     */
    config.network.listen_addresses = parse_listening_addresses(cli_args)?;

    config.beacon_nodes_tls_certs = cli_args.beacon_nodes_tls_certs.clone();
    config.execution_nodes_tls_certs = cli_args.execution_nodes_tls_certs.clone();

    /*
     * Http API server
     */
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

    /*
     * Prometheus metrics HTTP server
     */

    if cli_args.metrics {
        config.http_metrics.enabled = true;
    }

    if let Some(address) = cli_args.metrics_address {
        config.http_metrics.listen_addr = address;
    }

    if let Some(port) = cli_args.metrics_port {
        config.http_metrics.listen_port = port;
    }

    Ok(config)
}

/// Gets the listening_addresses for lighthouse based on the cli options.
pub fn parse_listening_addresses(cli_args: &Anchor) -> Result<ListenAddress, String> {
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
                warn!("When listening only over IPv6, use the --port flag. The value of --port6 will be ignored.");
            }

            if cli_args.discovery_port6.is_some() {
                warn!("When listening only over IPv6, use the --discovery-port flag. The value of --discovery-port6 will be ignored.")
            }

            if cli_args.quic_port6.is_some() {
                warn!("When listening only over IPv6, use the --quic-port flag. The value of --quic-port6 will be ignored.")
            }

            // use zero ports if required. If not, use the given port.
            let tcp_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_tcp6_port)
                .transpose()?
                .unwrap_or(cli_args.port);

            // use zero ports if required. If not, use the specific udp port. If none given, use
            // the tcp port.
            let disc_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_udp6_port)
                .transpose()?
                .or(cli_args.discovery_port)
                .unwrap_or(tcp_port);

            let quic_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_udp6_port)
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

            // use zero ports if required. If not, use the given port.
            let tcp_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_tcp4_port)
                .transpose()?
                .unwrap_or(cli_args.port);
            // use zero ports if required. If not, use the specific discovery port. If none given, use
            // the tcp port.
            let disc_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_udp4_port)
                .transpose()?
                .or(cli_args.discovery_port)
                .unwrap_or(tcp_port);
            // use zero ports if required. If not, use the specific quic port. If none given, use
            // the tcp port + 1.
            let quic_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_udp4_port)
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
                .then(unused_port::unused_tcp4_port)
                .transpose()?
                .unwrap_or(cli_args.port);
            let ipv4_disc_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_udp4_port)
                .transpose()?
                .or(cli_args.discovery_port)
                .unwrap_or(ipv4_tcp_port);
            let ipv4_quic_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_udp4_port)
                .transpose()?
                .or(cli_args.quic_port)
                .unwrap_or(if ipv4_tcp_port == 0 {
                    0
                } else {
                    ipv4_tcp_port + 1
                });

            let ipv6_tcp_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_tcp6_port)
                .transpose()?
                .unwrap_or(cli_args.port);
            let ipv6_disc_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_udp6_port)
                .transpose()?
                .or(cli_args.discovery_port6)
                .unwrap_or(ipv6_tcp_port);
            let ipv6_quic_port = cli_args
                .use_zero_ports
                .then(unused_port::unused_udp6_port)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // Ensures the default config does not panic.
    fn default_config() {
        Config::default();
    }
}
