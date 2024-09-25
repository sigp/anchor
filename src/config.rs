// use crate::{http_api, http_metrics};
use clap::ArgMatches;
// use clap_utils::{flags::DISABLE_MALLOC_TUNING_FLAG, parse_optional, parse_required};

use sensitive_url::SensitiveUrl;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

pub const DEFAULT_BEACON_NODE: &str = "http://localhost:5052/";
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
    /// beacon node is not synced at startup.
    pub allow_unsynced_beacon_node: bool,
    /// Configuration for the HTTP REST API.
    // TODO:
    // pub http_api: http_api::Config,
    /// Configuration for the HTTP REST API.
    // TODO:
    // pub http_metrics: http_metrics::Config,
    /// A list of custom certificates that the validator client will additionally use when
    /// connecting to a beacon node over SSL/TLS.
    pub beacon_nodes_tls_certs: Option<Vec<PathBuf>>,
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
        Self {
            data_dir,
            secrets_dir,
            beacon_nodes,
            allow_unsynced_beacon_node: false,
            // http_api: <_>::default(),
            // http_metrics: <_>::default(),
            beacon_nodes_tls_certs: None,
        }
    }
}

/// Returns a `Default` implementation of `Self` with some parameters modified by the supplied
/// `cli_args`.
pub fn from_cli(cli_args: &ArgMatches) -> Result<Config, String> {
    let mut config = Config::default();

    let default_root_dir = dirs::home_dir()
        .map(|home| home.join(DEFAULT_ROOT_DIR))
        .unwrap_or_else(|| PathBuf::from("."));

    let (mut data_dir, mut secrets_dir) = (None, None);
    if cli_args.get_one::<String>("datadir").is_some() {
        let temp_data_dir: PathBuf = parse_required(cli_args, "datadir")?;
        secrets_dir = Some(temp_data_dir.join(DEFAULT_SECRETS_DIR));
        data_dir = Some(temp_data_dir);
    };

    if cli_args.get_one::<String>("secrets-dir").is_some() {
        secrets_dir = Some(parse_required(cli_args, "secrets-dir")?);
    }

    config.data_dir = data_dir.unwrap_or_else(|| default_root_dir.join(DEFAULT_ROOT_DIR));

    config.secrets_dir = secrets_dir.unwrap_or_else(|| default_root_dir.join(DEFAULT_SECRETS_DIR));

    if !config.data_dir.exists() {
        fs::create_dir_all(&config.data_dir)
            .map_err(|e| format!("Failed to create {:?}: {:?}", config.data_dir, e))?;
    }

    if let Some(beacon_nodes) = parse_optional::<String>(cli_args, "beacon-nodes")? {
        config.beacon_nodes = beacon_nodes
            .split(',')
            .map(SensitiveUrl::parse)
            .collect::<Result<_, _>>()
            .map_err(|e| format!("Unable to parse beacon node URL: {:?}", e))?;
    }

    if let Some(tls_certs) = parse_optional::<String>(cli_args, "beacon-nodes-tls-certs")? {
        config.beacon_nodes_tls_certs = Some(tls_certs.split(',').map(PathBuf::from).collect());
    }

    /*
     * Http API server
     */
    // TODO:

    /*
    if cli_args.get_flag("http") {
        config.http_api.enabled = true;
    }

    if let Some(address) = cli_args.get_one::<String>("http-address") {
        if cli_args.get_flag("unencrypted-http-transport") {
            config.http_api.listen_addr = address
                .parse::<IpAddr>()
                .map_err(|_| "http-address is not a valid IP address.")?;
        } else {
            return Err(
                "While using `--http-address`, you must also use `--unencrypted-http-transport`."
                    .to_string(),
            );
        }
    }

    if let Some(port) = cli_args.get_one::<String>("http-port") {
        config.http_api.listen_port = port
            .parse::<u16>()
            .map_err(|_| "http-port is not a valid u16.")?;
    }

    if let Some(allow_origin) = cli_args.get_one::<String>("http-allow-origin") {
        // Pre-validate the config value to give feedback to the user on node startup, instead of
        // as late as when the first API response is produced.
        hyper::header::HeaderValue::from_str(allow_origin)
            .map_err(|_| "Invalid allow-origin value")?;

        config.http_api.allow_origin = Some(allow_origin.to_string());
    }
    */

    /*
     * Prometheus metrics HTTP server
     */

    // TODO:
    /*
    if cli_args.get_flag("metrics") {
        config.http_metrics.enabled = true;
    }

    if let Some(address) = cli_args.get_one::<String>("metrics-address") {
        config.http_metrics.listen_addr = address
            .parse::<IpAddr>()
            .map_err(|_| "metrics-address is not a valid IP address.")?;
    }

    if let Some(port) = cli_args.get_one::<String>("metrics-port") {
        config.http_metrics.listen_port = port
            .parse::<u16>()
            .map_err(|_| "metrics-port is not a valid u16.")?;
    }

    if let Some(allow_origin) = cli_args.get_one::<String>("metrics-allow-origin") {
        // Pre-validate the config value to give feedback to the user on node startup, instead of
        // as late as when the first API response is produced.
        hyper::header::HeaderValue::from_str(allow_origin)
            .map_err(|_| "Invalid allow-origin value")?;

        config.http_metrics.allow_origin = Some(allow_origin.to_string());
    }

    if cli_args.get_flag(DISABLE_MALLOC_TUNING_FLAG) {
        config.http_metrics.allocator_metrics_enabled = false;
    }
    */

    Ok(config)
}

// Helper functions for handling CLAP arguments

/// Returns the value of `name` or an error if it is not in `matches` or does not parse
/// successfully using `std::string::FromStr`.
pub fn parse_required<T>(matches: &ArgMatches, name: &str) -> Result<T, String>
where
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    parse_optional(matches, name)?.ok_or_else(|| format!("{} not specified", name))
}

/// Returns the value of `name` (if present) or an error if it does not parse successfully using
/// `std::string::FromStr`.
pub fn parse_optional<T>(matches: &ArgMatches, name: &str) -> Result<Option<T>, String>
where
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    matches
        .try_get_one::<String>(name)
        .map_err(|e| format!("Unable to parse {}: {}", name, e))?
        .map(|val| {
            val.parse()
                .map_err(|e| format!("Unable to parse {}: {}", name, e))
        })
        .transpose()
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
