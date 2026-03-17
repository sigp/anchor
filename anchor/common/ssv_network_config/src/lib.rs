use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
    str::FromStr,
};

use alloy::primitives::Address;
use enr::{CombinedKey, Enr};
use eth2_network_config::Eth2NetworkConfig;
// Re-export fork types for convenience
pub use fork::{ALAN_TOPIC_PREFIX, FORK_PREPARATION_EPOCHS, Fork, ForkConfig, ForkSchedule};
use serde::Deserialize;
use ssv_types::domain_type::DomainType;
use types::Epoch;

/// Immutable name of an SSV network (e.g., "mainnet", "holesky").
///
/// This represents the network identity that is constant throughout the network's lifetime.
/// It identifies which SSV network we're connected to.
///
/// For built-in networks (mainnet, holesky, hoodi), the name matches the network name.
/// For custom networks loaded via `--testnet-dir`, the name comes from `ssv_network_name.txt`.
pub type SsvNetworkName = String;

/// Configuration for a single fork as stored in the YAML file.
/// This is separate from `fork::ForkConfig` which uses typed `Epoch` and `DomainType`.
#[derive(Debug, Deserialize)]
struct ForkConfigYaml {
    epoch: u64,
    domain_type: String,
}

/// Type alias for deserializing fork schedule from YAML configuration files.
/// The YAML file maps fork names directly to their configuration.
type ForkScheduleFile = std::collections::HashMap<Fork, ForkConfigYaml>;

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
    pub fork_schedule: ForkSchedule,
    /// SSV network name (immutable identifier).
    pub network_name: SsvNetworkName,
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
        let ssv_domain_type: DomainType = domain_type
            .parse()
            .map_err(|e| format!("Unable to parse built-in domain type: {e}"))?;
        let fork_schedule_file: ForkScheduleFile = serde_yaml::from_str(fork_schedule_yaml)
            .map_err(|e| format!("Unable to parse built-in fork schedule: {e}"))?;
        let fork_schedule = Self::parse_fork_schedule(fork_schedule_file, ssv_domain_type, name)?;
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
            fork_schedule,
            network_name: SsvNetworkName::from(name),
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

        // Load domain type (required) - this is the baseline domain type for Alan fork
        let ssv_domain_type: DomainType = read(&base_dir.join("ssv_domain_type.txt"))?;

        // Load SSV network name (required) - used for topic prefixes
        let network_name: String = read(&base_dir.join("ssv_network_name.txt"))?;

        // Load fork schedule from YAML file, or use default if not present
        let fork_schedule_path = base_dir.join("ssv_fork_schedule.yaml");
        let fork_schedule = if fork_schedule_path.exists() {
            let file = File::open(&fork_schedule_path)
                .map_err(|e| format!("Unable to read {fork_schedule_path:?}: {e}"))?;
            let schedule_file: ForkScheduleFile = serde_yaml::from_reader(file)
                .map_err(|e| format!("Unable to parse {fork_schedule_path:?}: {e}"))?;
            Self::parse_fork_schedule(schedule_file, ssv_domain_type, &network_name)?
        } else {
            // Default to Alan fork only if no schedule file exists
            ForkSchedule::new(Fork::Alan, ssv_domain_type, &network_name)
        };

        // Load eth2 network config
        let eth2_network = Self::load_eth2_network_config(&base_dir)?;

        Ok(Self {
            ssv_boot_nodes,
            ssv_contract: read(&base_dir.join("ssv_contract_address.txt"))?,
            ssv_contract_block: read(&base_dir.join("ssv_contract_block.txt"))?,
            eth2_network,
            fork_schedule,
            network_name: SsvNetworkName::from(network_name),
        })
    }

    /// Parse fork schedule file into ForkSchedule.
    ///
    /// The YAML file contains non-Alan forks with their epochs and domain types.
    /// Alan fork is always added at epoch 0 with the baseline domain type.
    fn parse_fork_schedule(
        forks: ForkScheduleFile,
        baseline_domain_type: DomainType,
        network_name: &str,
    ) -> Result<ForkSchedule, String> {
        let mut configs = BTreeMap::new();

        // Alan is always at epoch 0 with the baseline domain type
        configs.insert(Fork::Alan, (Epoch::new(0), baseline_domain_type));

        // Add forks from YAML file
        for (fork, yaml_config) in forks {
            let domain_type: DomainType = yaml_config
                .domain_type
                .parse()
                .map_err(|e| format!("Invalid domain type for fork {fork}: {e}"))?;
            configs.insert(fork, (Epoch::new(yaml_config.epoch), domain_type));
        }

        ForkSchedule::from_fork_configs(configs, network_name)
    }

    /// Load eth2 network config from the testnet directory.
    ///
    /// If "ssv_eth_network.txt" exists, it specifies which built-in Ethereum network
    /// to use (e.g., "mainnet", "holesky"). Otherwise, loads the eth2 config from
    /// individual files in the directory.
    fn load_eth2_network_config(base_dir: &Path) -> Result<Eth2NetworkConfig, String> {
        let ssv_eth_network_path = base_dir.join("ssv_eth_network.txt");
        if ssv_eth_network_path.exists() {
            let eth_network_name: String = read(&ssv_eth_network_path)?;
            Eth2NetworkConfig::constant(&eth_network_name)?.ok_or_else(|| {
                format!("Hardcoded network '{eth_network_name}' specified in ssv_eth_network.txt is unknown")
            })
        } else {
            Eth2NetworkConfig::load(base_dir.to_path_buf())
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

    use fork::ALAN_TOPIC_PREFIX;
    use tempfile::TempDir;

    use super::*;

    // Built-in network names
    const MAINNET: &str = "mainnet";
    const HOLESKY: &str = "holesky";
    const HOODI: &str = "hoodi";

    // Test network configuration
    const TEST_NETWORK_NAME: &str = "test-network";
    const TEST_CONTRACT_ADDRESS: &str = "0x38A4794cCEd47d3baf7370CcC43B560D3a1beEFA";
    const TEST_CONTRACT_BLOCK: &str = "123456";
    const TEST_DOMAIN_TYPE_STR: &str = "00000001";
    const TEST_DOMAIN_TYPE: DomainType = DomainType([0, 0, 0, 1]);
    const BOOLE_DOMAIN_TYPE: DomainType = DomainType([0, 0, 0, 2]);

    // Epoch constants for fork schedule tests
    const ALAN_EPOCH: u64 = 0;
    const BEFORE_BOOLE_EPOCH: u64 = 99;
    const BOOLE_FORK_EPOCH: u64 = 100;
    const AFTER_BOOLE_EPOCH: u64 = 1000;
    const LARGE_BOOLE_EPOCH: u64 = 12500;

    /// Construct expected Boole topic prefix for a network.
    fn expected_boole_prefix(network: &str) -> String {
        format!("/ssv/{}/boole/", network)
    }

    /// Asserts that a config has a valid fork schedule with Alan at epoch 0.
    fn assert_valid_fork_schedule(config: &SsvNetworkConfig) {
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Alan),
            Some(Epoch::new(ALAN_EPOCH)),
            "Alan must always be at epoch 0"
        );
    }

    /// Creates a minimal network config directory for testing.
    fn create_test_config_dir(fork_schedule_yaml: Option<&str>) -> TempDir {
        let dir = TempDir::new().unwrap();

        std::fs::write(
            dir.path().join("ssv_contract_address.txt"),
            TEST_CONTRACT_ADDRESS,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("ssv_contract_block.txt"),
            TEST_CONTRACT_BLOCK,
        )
        .unwrap();
        std::fs::write(dir.path().join("ssv_domain_type.txt"), TEST_DOMAIN_TYPE_STR).unwrap();
        std::fs::write(dir.path().join("ssv_network_name.txt"), TEST_NETWORK_NAME).unwrap();
        std::fs::write(dir.path().join("ssv_eth_network.txt"), MAINNET).unwrap();

        if let Some(yaml) = fork_schedule_yaml {
            let mut file =
                std::fs::File::create(dir.path().join("ssv_fork_schedule.yaml")).unwrap();
            file.write_all(yaml.as_bytes()).unwrap();
        }

        dir
    }

    // ==================== Built-in network config tests ====================

    #[test]
    fn test_constant_holesky_loads_valid_config() {
        // Act
        let config = SsvNetworkConfig::constant(HOLESKY).unwrap().unwrap();

        // Assert
        assert_valid_fork_schedule(&config);
    }

    #[test]
    fn test_constant_hoodi_loads_valid_config() {
        // Act
        let config = SsvNetworkConfig::constant(HOODI).unwrap().unwrap();

        // Assert
        assert_valid_fork_schedule(&config);
    }

    #[test]
    fn test_constant_mainnet_loads_valid_config() {
        // Act
        let config = SsvNetworkConfig::constant(MAINNET).unwrap().unwrap();

        // Assert
        assert_valid_fork_schedule(&config);
    }

    #[test]
    fn test_constant_networks_have_correct_network_names() {
        // Arrange & Act & Assert
        let test_cases = [(MAINNET, MAINNET), (HOLESKY, HOLESKY), (HOODI, HOODI)];

        for (network, expected_name) in test_cases {
            let config = SsvNetworkConfig::constant(network).unwrap().unwrap();
            assert_eq!(
                config.network_name.as_str(),
                expected_name,
                "Network {} should have name {}",
                network,
                expected_name
            );
        }
    }

    // ==================== Config loading tests ====================

    #[test]
    fn test_load_parses_fork_schedule_with_domain_types() {
        // Arrange
        let yaml = format!(
            r#"
boole:
  epoch: {}
  domain_type: "00000002"
"#,
            LARGE_BOOLE_EPOCH
        );
        let dir = create_test_config_dir(Some(&yaml));

        // Act
        let config = SsvNetworkConfig::load(dir.path().to_path_buf()).unwrap();

        // Assert - fork schedule contains both epochs and domain types
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Alan),
            Some(Epoch::new(ALAN_EPOCH))
        );
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Boole),
            Some(Epoch::new(LARGE_BOOLE_EPOCH))
        );
        assert_eq!(
            config.fork_schedule.domain_type(Fork::Alan),
            Some(TEST_DOMAIN_TYPE)
        );
        assert_eq!(
            config.fork_schedule.domain_type(Fork::Boole),
            Some(BOOLE_DOMAIN_TYPE)
        );
    }

    #[test]
    fn test_load_with_empty_fork_schedule_only_has_alan() {
        // Arrange
        let dir = create_test_config_dir(Some("{}"));

        // Act
        let config = SsvNetworkConfig::load(dir.path().to_path_buf()).unwrap();

        // Assert
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Alan),
            Some(Epoch::new(ALAN_EPOCH))
        );
        assert_eq!(config.fork_schedule.fork_epoch(Fork::Boole), None);
        assert_eq!(
            config.fork_schedule.domain_type(Fork::Alan),
            Some(TEST_DOMAIN_TYPE)
        );
    }

    #[test]
    fn test_load_without_fork_schedule_file_defaults_to_alan_only() {
        // Arrange
        let dir = create_test_config_dir(None);

        // Act
        let config = SsvNetworkConfig::load(dir.path().to_path_buf()).unwrap();

        // Assert
        assert_eq!(
            config.fork_schedule.fork_epoch(Fork::Alan),
            Some(Epoch::new(ALAN_EPOCH))
        );
        assert_eq!(config.fork_schedule.fork_epoch(Fork::Boole), None);
    }

    #[test]
    fn test_load_returns_error_on_invalid_yaml() {
        // Arrange
        let dir = create_test_config_dir(Some("this is not valid yaml: ["));

        // Act
        let result = SsvNetworkConfig::load(dir.path().to_path_buf());

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn test_load_returns_error_when_network_name_file_missing() {
        // Arrange: Create config dir without ssv_network_name.txt
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("ssv_contract_address.txt"),
            TEST_CONTRACT_ADDRESS,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("ssv_contract_block.txt"),
            TEST_CONTRACT_BLOCK,
        )
        .unwrap();
        std::fs::write(dir.path().join("ssv_domain_type.txt"), TEST_DOMAIN_TYPE_STR).unwrap();
        std::fs::write(dir.path().join("ssv_eth_network.txt"), MAINNET).unwrap();

        // Act
        let result = SsvNetworkConfig::load(dir.path().to_path_buf());

        // Assert
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("ssv_network_name.txt"),
            "Error should mention missing file"
        );
    }

    #[test]
    fn test_load_uses_network_name_from_file() {
        // Arrange
        let dir = create_test_config_dir(None);

        // Act
        let config = SsvNetworkConfig::load(dir.path().to_path_buf()).unwrap();

        // Assert
        assert_eq!(config.network_name.as_str(), TEST_NETWORK_NAME);
        assert_eq!(
            Fork::Boole.topic_prefix(config.network_name.as_str()),
            expected_boole_prefix(TEST_NETWORK_NAME)
        );
    }

    // ==================== Topic prefix tests ====================

    #[test]
    fn test_topic_prefix_for_fork_alan_returns_legacy_prefix() {
        // Arrange
        let config = SsvNetworkConfig::constant(MAINNET).unwrap().unwrap();

        // Act
        let result = Fork::Alan.topic_prefix(config.network_name.as_str());

        // Assert
        assert_eq!(result, ALAN_TOPIC_PREFIX);
    }

    #[test]
    fn test_topic_prefix_for_fork_boole_returns_network_specific_prefix() {
        // Arrange & Act & Assert
        let test_cases = [
            (MAINNET, expected_boole_prefix(MAINNET)),
            (HOLESKY, expected_boole_prefix(HOLESKY)),
        ];

        for (network, expected_prefix) in test_cases {
            let config = SsvNetworkConfig::constant(network).unwrap().unwrap();
            let result = Fork::Boole.topic_prefix(config.network_name.as_str());
            assert_eq!(result, expected_prefix);
        }
    }

    /// Tests that topic prefix changes correctly based on active fork at different epochs.
    #[test]
    fn test_topic_prefix_changes_with_active_fork_at_epoch() {
        // Arrange
        let yaml = format!(
            r#"
boole:
  epoch: {}
  domain_type: "00000002"
"#,
            BOOLE_FORK_EPOCH
        );
        let dir = create_test_config_dir(Some(&yaml));
        let config = SsvNetworkConfig::load(dir.path().to_path_buf()).unwrap();
        let network_name = config.network_name.as_str();

        // Act & Assert: Before Boole activation - should use Alan prefix
        let active_fork = config.fork_schedule.active_fork(Epoch::new(ALAN_EPOCH));
        assert_eq!(active_fork.topic_prefix(network_name), ALAN_TOPIC_PREFIX);

        let active_fork = config
            .fork_schedule
            .active_fork(Epoch::new(BEFORE_BOOLE_EPOCH));
        assert_eq!(active_fork.topic_prefix(network_name), ALAN_TOPIC_PREFIX);

        // Act & Assert: At and after Boole activation - should use Boole prefix
        let active_fork = config
            .fork_schedule
            .active_fork(Epoch::new(BOOLE_FORK_EPOCH));
        assert_eq!(
            active_fork.topic_prefix(network_name),
            expected_boole_prefix(TEST_NETWORK_NAME)
        );

        let active_fork = config
            .fork_schedule
            .active_fork(Epoch::new(AFTER_BOOLE_EPOCH));
        assert_eq!(
            active_fork.topic_prefix(network_name),
            expected_boole_prefix(TEST_NETWORK_NAME)
        );
    }

    // ==================== SsvNetworkName tests ====================

    #[test]
    fn test_topic_prefix_for_fork_works_with_network_name() {
        // Arrange
        const TESTNET: &str = "testnet";
        let network_name: SsvNetworkName = TESTNET.to_string();

        // Act & Assert
        assert_eq!(Fork::Alan.topic_prefix(&network_name), ALAN_TOPIC_PREFIX);
        assert_eq!(
            Fork::Boole.topic_prefix(&network_name),
            expected_boole_prefix(TESTNET)
        );
    }
}
