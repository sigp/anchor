use client::config::Config;
use client::Client;
use lighthouse_network::{ListenAddr, ListenAddress};
use network::load_enr_from_disk;
use network::Enr;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use task_executor::TaskExecutor;
use tracing::{info, warn};

use network::{DEFAULT_DISC_PORT, DEFAULT_IPV4_ADDRESS, DEFAULT_QUIC_PORT, DEFAULT_TCP_PORT};

pub struct LocalAnchorNode {
    pub config: Config,
}

impl LocalAnchorNode {
    // Create a new local anchor node
    pub fn new(index: u16, mut anchor_config: Config) -> Self {
        // Access pre-populated database for the operator
        // The data dir for each operator is stored in mock-data/operator-{index}
        let data_path = PathBuf::from("mock-data");
        let data_dir = format!("operator-{}", index);
        anchor_config.data_dir = data_path.join(data_dir);

        // Make sure to cleanup previously slashing databases. We dont care about the return value
        // as it is a static location and it either exists and will be removed, or does not exist
        // and nothing will hapenn
        let _ = std::fs::remove_file(anchor_config.data_dir.join("slashing_protection.sqlite"));
        let _ = std::fs::remove_file(
            anchor_config
                .data_dir
                .join("slashing_protection.sqlite-journal"),
        );

        // Setup the network dir
        anchor_config.network.network_dir = anchor_config.data_dir.join("network");

        // Configure ports
        // subtract index to prevent collision with quic port
        // index == 0 defines boot node
        let libp2p_tcp_port = DEFAULT_TCP_PORT - index;
        let discv5_port = DEFAULT_DISC_PORT - index;

        anchor_config.network.listen_addresses = ListenAddress::V4(ListenAddr {
            addr: DEFAULT_IPV4_ADDRESS,
            disc_port: discv5_port,
            quic_port: DEFAULT_QUIC_PORT + index,
            tcp_port: libp2p_tcp_port,
        });

        anchor_config.network.enr_udp4_port = Some(discv5_port.try_into().unwrap());
        anchor_config.network.enr_tcp4_port = Some(libp2p_tcp_port.try_into().unwrap());
        anchor_config.network.enr_address = (Some(Ipv4Addr::LOCALHOST), None);
        anchor_config.network.disable_quic_support = true;

        Self {
            config: anchor_config,
        }
    }

    // Enr is saved to disk, fetch it
    pub fn get_enr(&self) -> Enr {
        loop {
            if let Some(enr) = load_enr_from_disk(&self.config.network.network_dir) {
                return enr;
            }
        }
    }

    // Run the anchor node with the given executor
    pub fn run(&mut self, executor: TaskExecutor) -> Result<(), String> {
        // Clone necessary data for the async task
        let config = self.config.clone();

        let executor_clone = executor.clone();
        executor.spawn(
            async move {
                match Client::run::<types::MainnetEthSpec>(executor_clone, config).await {
                    Ok(_) => {
                        info!("Anchor node completed successfully");
                    }
                    Err(e) => {
                        warn!("Anchor node  failed: {}", e);
                    }
                }
            },
            "anchor node",
        );

        Ok(())
    }
}
