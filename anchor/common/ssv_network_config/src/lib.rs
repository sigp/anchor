use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    str::FromStr,
};

use alloy::primitives::Address;
use enr::{CombinedKey, Enr};
use eth2_network_config::Eth2NetworkConfig;
// Re-export fork types for convenience
pub use fork::{FORK_PREPARATION_EPOCHS, Fork, ForkSchedule};
use serde::Deserialize;
use ssv_types::domain_type::DomainType;

/// Structure for deserializing fork schedule from YAML configuration files.
#[derive(Debug, Deserialize)]
struct ForkScheduleFile {
    forks: HashMap<Fork, u64>,
}

macro_rules! include_str_for_net {
    ($network:ident, $file:literal) => {
        include_str!(concat!(
            "../built_in_network_configs/",
            stringify!($network),
            "/",
            $file
        ))
    };
}

macro_rules! get_hardcoded {
    ($network:ident) => {
        (
            include_str_for_net!($network, "ssv_boot_enr.yaml"),
            include_str_for_net!($network, "ssv_contract_address.txt"),
            include_str_for_net!($network, "ssv_contract_block.txt"),
            include_str_for_net!($network, "ssv_domain_type.txt"),
            include_str_for_net!($network, "ssv_fork_schedule.yaml"),
        )
    };
}

#[derive(Clone, Debug)]
pub struct SsvNetworkConfig {
    pub eth2_network: Eth2NetworkConfig,
    pub ssv_boot_nodes: Option<Vec<Enr<CombinedKey>>>,
    pub ssv_contract: Address,
    pub ssv_contract_block: u64,
    pub ssv_domain_type: DomainType,
    pub fork_schedule: ForkSchedule,
}

impl SsvNetworkConfig {
    pub fn constant(name: &str) -> Result<Option<Self>, String> {
        let (enr_yaml, address, block, domain_type, fork_schedule_yaml) = match name {
            "mainnet" => get_hardcoded!(mainnet),
            "holesky" => get_hardcoded!(holesky),
            "hoodi" => get_hardcoded!(hoodi),
            _ => return Ok(None),
        };
        let Some(eth2_network) = Eth2NetworkConfig::constant(name)? else {
            return Ok(None);
        };
        let fork_schedule_file: ForkScheduleFile = serde_yaml::from_str(fork_schedule_yaml)
            .map_err(|e| format!("Unable to parse built-in fork schedule: {e}"))?;
        Ok(Some(Self {
            eth2_network,
            ssv_boot_nodes: Some(
                serde_yaml::from_str(enr_yaml).map_err(|_| "Unable to parse built-in yaml!")?,
            ),
            ssv_contract: address
                .parse()
                .map_err(|_| "Unable to parse built-in address!")?,
            ssv_contract_block: block
                .parse()
                .map_err(|_| "Unable to parse built-in block!")?,
            ssv_domain_type: domain_type
                .parse()
                .map_err(|e| format!("Unable to parse built-in domain type: {e}"))?,
            fork_schedule: ForkSchedule::from_fork_epochs(fork_schedule_file.forks)?,
        }))
    }

    pub fn load(base_dir: PathBuf) -> Result<Self, String> {
        let ssv_boot_nodes_path = base_dir.join("ssv_boot_enr.yaml");
        let ssv_boot_nodes = ssv_boot_nodes_path
            .exists()
            .then(|| {
                File::open(&ssv_boot_nodes_path)
                    .map_err(|e| format!("Unable to read {ssv_boot_nodes_path:?}: {e}"))
                    .and_then(|f| {
                        serde_yaml::from_reader(f)
                            .map_err(|e| format!("Unable to parse {ssv_boot_nodes_path:?}: {e}"))
                    })
            })
            .transpose()?;

        // Load fork schedule from YAML file, or use default if not present
        let fork_schedule_path = base_dir.join("ssv_fork_schedule.yaml");
        let fork_schedule = if fork_schedule_path.exists() {
            let file = File::open(&fork_schedule_path)
                .map_err(|e| format!("Unable to read {fork_schedule_path:?}: {e}"))?;
            let schedule_file: ForkScheduleFile = serde_yaml::from_reader(file)
                .map_err(|e| format!("Unable to parse {fork_schedule_path:?}: {e}"))?;
            ForkSchedule::from_fork_epochs(schedule_file.forks)?
        } else {
            // Default to Alan fork if no schedule file exists
            ForkSchedule::default()
        };

        Ok(Self {
            ssv_boot_nodes,
            ssv_contract: read(&base_dir.join("ssv_contract_address.txt"))?,
            ssv_contract_block: read(&base_dir.join("ssv_contract_block.txt"))?,
            ssv_domain_type: read(&base_dir.join("ssv_domain_type.txt"))?,
            eth2_network: Self::load_eth2_network_config(base_dir)?,
            fork_schedule,
        })
    }

    /// If a hardcoded eth network is specified in "ssv_eth_network.txt", use it, else try to load
    /// its definition from files.
    fn load_eth2_network_config(base_dir: PathBuf) -> Result<Eth2NetworkConfig, String> {
        let ssv_eth_network_path = base_dir.join("ssv_eth_network.txt");
        if ssv_eth_network_path.exists() {
            let network_name: String = read(&ssv_eth_network_path)?;
            Eth2NetworkConfig::constant(&network_name).and_then(|network_config| {
                network_config.ok_or_else(|| {
                    "Hardcoded network specified in ssv_eth_network.txt is unknown".to_string()
                })
            })
        } else {
            Eth2NetworkConfig::load(base_dir)
        }
    }
}

fn read<T: FromStr>(file: &Path) -> Result<T, String> {
    std::fs::read_to_string(file)
        .map_err(|e| format!("Unable to read {file:?}: {e}"))?
        .trim()
        .parse()
        .map_err(|_| format!("Unable to parse {file:?}"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;

    use super::*;

    fn assert_valid_fork_schedule(config: &SsvNetworkConfig) {
        // Alan must always be at epoch 0
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Alan),
            Some(types::Epoch::new(0))
        );
    }

    #[test]
    fn test_holesky() {
        let config = SsvNetworkConfig::constant("holesky").unwrap().unwrap();
        assert_valid_fork_schedule(&config);
    }

    #[test]
    fn test_hoodi() {
        let config = SsvNetworkConfig::constant("hoodi").unwrap().unwrap();
        assert_valid_fork_schedule(&config);
    }

    #[test]
    fn test_mainnet() {
        let config = SsvNetworkConfig::constant("mainnet").unwrap().unwrap();
        assert_valid_fork_schedule(&config);
    }

    /// Helper to create a minimal network config directory for testing
    fn create_test_config_dir(fork_schedule_yaml: Option<&str>) -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create required files
        std::fs::write(
            dir.path().join("ssv_contract_address.txt"),
            "0x38A4794cCEd47d3baf7370CcC43B560D3a1beEFA",
        )
        .unwrap();
        std::fs::write(dir.path().join("ssv_contract_block.txt"), "123456").unwrap();
        std::fs::write(dir.path().join("ssv_domain_type.txt"), "00000001").unwrap();

        // Create ssv_eth_network.txt to use a hardcoded network
        std::fs::write(dir.path().join("ssv_eth_network.txt"), "mainnet").unwrap();

        // Optionally create fork schedule
        if let Some(yaml) = fork_schedule_yaml {
            let mut file =
                std::fs::File::create(dir.path().join("ssv_fork_schedule.yaml")).unwrap();
            file.write_all(yaml.as_bytes()).unwrap();
        }

        dir
    }

    #[test]
    fn test_load_with_fork_schedule() {
        let dir = create_test_config_dir(Some("forks:\n  boole: 12500"));

        let config = SsvNetworkConfig::load(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Alan),
            Some(types::Epoch::new(0))
        );
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Boole),
            Some(types::Epoch::new(12500))
        );
    }

    #[test]
    fn test_load_with_empty_forks() {
        let dir = create_test_config_dir(Some("forks: {}"));

        let config = SsvNetworkConfig::load(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Alan),
            Some(types::Epoch::new(0))
        );
        assert_eq!(config.fork_schedule.fork_epoch(Fork::Boole), None);
    }

    #[test]
    fn test_load_without_fork_schedule_file() {
        let dir = create_test_config_dir(None);

        let config = SsvNetworkConfig::load(dir.path().to_path_buf()).unwrap();

        // Should fall back to default (Alan at epoch 0)
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Alan),
            Some(types::Epoch::new(0))
        );
        assert_eq!(config.fork_schedule.fork_epoch(Fork::Boole), None);
    }

    #[test]
    fn test_load_with_invalid_yaml() {
        let dir = create_test_config_dir(Some("this is not valid yaml: ["));

        let result = SsvNetworkConfig::load(dir.path().to_path_buf());
        assert!(result.is_err());
    }
}
