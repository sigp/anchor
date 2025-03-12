use client::config::Config;
use lighthouse_network::{ListenAddr, ListenAddress};
use std::path::PathBuf;

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

        Self {
            config: anchor_config,
        }
    }

    pub fn get_enr(&self) -> String {
        todo!()
    }

    /*
    // Run the anchor node with the given executor
    pub fn run(&mut self, executor: TaskExecutor) -> Result<(), String> {
        // Clone necessary data for the async task
        let config = self.config.clone();

        executor.spawn(
            async move {
                match Client::run(executor, config).await {
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
    */
}
