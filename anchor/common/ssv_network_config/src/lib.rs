use std::{
    fs::File,
    path::{Path, PathBuf},
    str::FromStr,
};

use alloy::primitives::Address;
use enr::{CombinedKey, Enr};
use eth2_network_config::Eth2NetworkConfig;
use ssv_types::domain_type::DomainType;

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
}

impl SsvNetworkConfig {
    pub fn constant(name: &str) -> Result<Option<Self>, String> {
        let (enr_yaml, address, block, domain_type) = match name {
            "mainnet" => get_hardcoded!(mainnet),
            "holesky" => get_hardcoded!(holesky),
            "hoodi" => get_hardcoded!(hoodi),
            _ => return Ok(None),
        };
        let Some(eth2_network) = Eth2NetworkConfig::constant(name)? else {
            return Ok(None);
        };
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
                .map_err(|e| format!("Unable to parse built-in domain type: {}", e))?,
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

        Ok(Self {
            ssv_boot_nodes,
            ssv_contract: read(&base_dir.join("ssv_contract_address.txt"))?,
            ssv_contract_block: read(&base_dir.join("ssv_contract_block.txt"))?,
            ssv_domain_type: read(&base_dir.join("ssv_domain_type.txt"))?,
            eth2_network: Self::load_eth2_network_config(base_dir)?,
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
        .parse()
        .map_err(|_| format!("Unable to parse {file:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_holesky() {
        SsvNetworkConfig::constant("holesky").unwrap().unwrap();
    }

    #[test]
    fn test_hoodi() {
        SsvNetworkConfig::constant("hoodi").unwrap().unwrap();
    }

    #[test]
    fn test_mainnet() {
        SsvNetworkConfig::constant("mainnet").unwrap().unwrap();
    }
}
