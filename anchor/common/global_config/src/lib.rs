pub mod data_dir;

use std::{path::PathBuf, str::FromStr, sync::Arc};

use clap::Parser;
use ssv_network_config::SsvNetworkConfig;
use tracing::Level;

use crate::data_dir::DataDir;

/// Default network, used to partition the data storage
pub const DEFAULT_HARDCODED_NETWORK: &str = "mainnet";

/// Config that applies to all subcommands: The resolved network and datadir. This avoids repeated
/// logic matching the datadir from the actual CLI definition.
#[derive(Debug, Clone)]
pub struct GlobalConfig {
    pub data_dir: Arc<DataDir>,
    pub ssv_network: SsvNetworkConfig,
    pub debug_level: Level,
}

#[derive(Parser, Clone, Debug)]
pub struct GlobalFlags {
    #[clap(
        long,
        short = 'd',
        global = true,
        value_name = "DIR",
        help = "Used to specify a custom root data directory for the Anchor key and database. \
                Defaults to $HOME/.anchor/{network} where network is the value of the `network` flag \
                Note: Users should specify separate custom datadirs for different networks.",
        display_order = 0,
        alias = "datadir"
    )]
    pub data_dir: Option<PathBuf>,

    #[clap(
        long,
        short = 't',
        global = true,
        value_name = "DIR",
        help = "Path to directory containing eth2_testnet specs.",
        display_order = 0
    )]
    pub testnet_dir: Option<PathBuf>,

    #[clap(
        long,
        global = true,
        value_name = "NETWORK",
        value_parser = vec!["mainnet", "holesky", "hoodi"],
        conflicts_with = "testnet_dir",
        help = "Name of the chain Anchor will validate.",
        display_order = 0,
        default_value = DEFAULT_HARDCODED_NETWORK,
    )]
    pub network: String,

    #[arg(
        long,
        global = true,
        default_value_t = Level::INFO,
        value_parser = Level::from_str,
        help = "Specifies the verbosity level used when emitting logs to the terminal")]
    pub debug_level: Level,
}

impl TryFrom<&GlobalFlags> for GlobalConfig {
    type Error = String;

    fn try_from(cli: &GlobalFlags) -> Result<Self, Self::Error> {
        let ssv_network = if let Some(testnet_dir) = &cli.testnet_dir {
            SsvNetworkConfig::load(testnet_dir.clone())
        } else {
            SsvNetworkConfig::constant(&cli.network)
                .and_then(|net| net.ok_or_else(|| format!("Unknown network {}", cli.network)))
        }?;

        let data_dir = if let Some(data_dir) = &cli.data_dir {
            DataDir::new(data_dir.clone())
        } else {
            DataDir::default_for_network(&ssv_network)
        }
        .map_err(|e| e.to_string())?;

        Ok(GlobalConfig {
            data_dir: Arc::new(data_dir),
            ssv_network,
            debug_level: cli.debug_level,
        })
    }
}
