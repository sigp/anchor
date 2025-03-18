use crate::local_anchor_node::LocalAnchorNode;
use crate::util::{default_anchor_config, default_client_config, default_mock_execution_config};
use client::config::Config as AnchorConfig;
use node_test_rig::{
    environment::RuntimeContext,
    eth2::{types::EthSpec, BeaconNodeHttpClient, SensitiveUrl as Eth2SensitiveUrl},
    ClientConfig, LocalBeaconNode, LocalExecutionNode, MockExecutionConfig,
};
use sensitive_url::SensitiveUrl;
use std::ops::Deref;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BOOTNODE_PORT: u16 = 42424;
const QUIC_PORT: u16 = 43424;
pub const EXECUTION_PORT: u16 = 4000;

pub struct SsvLocalNetwork<E: EthSpec> {
    pub inner: Arc<Inner<E>>,
}

impl<E: EthSpec> Clone for SsvLocalNetwork<E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<E: EthSpec> Deref for SsvLocalNetwork<E> {
    type Target = Inner<E>;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

pub struct Inner<E: EthSpec> {
    pub context: RuntimeContext<E>,
    pub beacon_nodes: RwLock<Vec<LocalBeaconNode<E>>>,
    pub proposer_nodes: RwLock<Vec<LocalBeaconNode<E>>>,
    pub execution_nodes: RwLock<Vec<LocalExecutionNode<E>>>,
    pub anchor_nodes: RwLock<Vec<LocalAnchorNode>>,
}

pub struct SsvNetworkParams {
    pub num_validators: usize,
    pub num_nodes: usize,
    pub num_proposers: usize,
    pub genesis_delay: u64,
}

impl<E: EthSpec> SsvLocalNetwork<E> {
    pub async fn create_local_network(
        network_params: SsvNetworkParams,
        context: RuntimeContext<E>,
    ) -> Result<
        (
            SsvLocalNetwork<E>,
            ClientConfig,
            MockExecutionConfig,
            AnchorConfig,
        ),
        String,
    > {
        let genesis_time: u64 = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "should get system time")?
            + Duration::from_secs(network_params.genesis_delay))
        .as_secs();

        let beacon_config = default_client_config(network_params, genesis_time);
        let execution_config =
            default_mock_execution_config::<E>(&context.eth2_config().spec, genesis_time);
        let anchor_config = default_anchor_config();

        let network = Self {
            inner: Arc::new(Inner {
                context,
                beacon_nodes: RwLock::new(Vec::new()),
                proposer_nodes: RwLock::new(Vec::new()),
                execution_nodes: RwLock::new(Vec::new()),
                anchor_nodes: RwLock::new(Vec::new()),
            }),
        };

        Ok((network, beacon_config, execution_config, anchor_config))
    }

    pub async fn add_anchor_node(
        &self,
        index: usize,
        mut anchor_config: AnchorConfig,
    ) -> Result<(), String> {
        {
            // Add ENR of bootnode
            let read_lock = self.anchor_nodes.read().expect("Failed to get read lock");
            let _boot_node = read_lock.first();
            // If we have a bootnode, add the enr

            /*if let Some(boot_node) = boot_node {
                let enr = Enr::from_str(&boot_node.get_enr()).unwrap();
                anchor_config.network.boot_nodes_enr.push(enr)
            }*/
        }

        // Add a beacon node endpoint
        let beacon_node = {
            let read_lock = self.beacon_nodes.read().expect("Failed to get read lock");
            let beacon_node = read_lock
                .get(index)
                .ok_or_else(|| format!("No beacon node for index {}", index))?;
            let beacon_node = beacon_node
                .client
                .http_api_listen_addr()
                .expect("Must have http started");
            SensitiveUrl::parse(
                format!("http://{}:{}", beacon_node.ip(), beacon_node.port()).as_str(),
            )
            .unwrap()
        };
        anchor_config.beacon_nodes = vec![beacon_node];

        // Add a execution node endpoint
        let execution_addr =
            SensitiveUrl::parse(&format!("http://localhost:{}", EXECUTION_PORT)).unwrap();
        anchor_config.execution_nodes.push(execution_addr);

        // Construct and run a new local anchor node
        let mut anchor_node = LocalAnchorNode::new(index as u16, anchor_config);
        anchor_node.run(self.context.executor.clone())?;

        // Add node to the network
        self.anchor_nodes
            .write()
            .expect("Failed to get write lock")
            .push(anchor_node);

        Ok(())
    }

    // Add a new beacon node to the local network
    pub async fn add_beacon_node(
        &self,
        mut beacon_config: ClientConfig,
        execution_config: MockExecutionConfig,
        is_proposer: bool,
    ) -> Result<(), String> {
        let first_bn_exists: bool;
        {
            // Add ENR of the bootnode if it exists
            let read_lock = self.beacon_nodes.read().expect("Failed to get read lock");
            let boot_node = read_lock.first();
            first_bn_exists = boot_node.is_some();
            if let Some(boot_node) = boot_node {
                // Modify beacon_config to add boot node details.
                beacon_config.network.boot_nodes_enr.push(
                    boot_node
                        .client
                        .enr()
                        .expect("Bootnode must have a network."),
                );
            }
        }
        let (beacon_node, execution_node) = if first_bn_exists {
            // Network already exists. We construct a new node.
            self.construct_beacon_node(beacon_config, execution_config, is_proposer)
                .await?
        } else {
            // Network does not exist. We construct a boot node.
            self.construct_boot_node(beacon_config, execution_config)
                .await?
        };

        // Add nodes to the network.
        self.execution_nodes
            .write()
            .expect("Failed to get write lock")
            .push(execution_node);
        if is_proposer {
            self.proposer_nodes
                .write()
                .expect("Failed to get write lock")
                .push(beacon_node);
        } else {
            self.beacon_nodes
                .write()
                .expect("Failed to get write lock")
                .push(beacon_node);
        }
        Ok(())
    }

    async fn construct_boot_node(
        &self,
        mut beacon_config: ClientConfig,
        mock_execution_config: MockExecutionConfig,
    ) -> Result<(LocalBeaconNode<E>, LocalExecutionNode<E>), String> {
        beacon_config.network.set_ipv4_listening_address(
            std::net::Ipv4Addr::UNSPECIFIED,
            BOOTNODE_PORT,
            BOOTNODE_PORT,
            QUIC_PORT,
        );

        beacon_config.network.enr_udp4_port = Some(BOOTNODE_PORT.try_into().expect("non zero"));
        beacon_config.network.enr_tcp4_port = Some(BOOTNODE_PORT.try_into().expect("non zero"));
        beacon_config.network.discv5_config.table_filter = |_| true;

        let execution_node = LocalExecutionNode::new(
            self.context.service_context("boot_node_el".into()),
            mock_execution_config,
        );

        beacon_config.execution_layer = Some(execution_layer::Config {
            execution_endpoint: Some(
                Eth2SensitiveUrl::parse(&execution_node.server.url()).unwrap(),
            ),
            default_datadir: execution_node.datadir.path().to_path_buf(),
            secret_file: Some(execution_node.datadir.path().join("jwt.hex")),
            ..Default::default()
        });

        let beacon_node = LocalBeaconNode::production(
            self.context.service_context("boot_node".into()),
            beacon_config,
        )
        .await?;

        Ok((beacon_node, execution_node))
    }

    async fn construct_beacon_node(
        &self,
        mut beacon_config: ClientConfig,
        mut mock_execution_config: MockExecutionConfig,
        is_proposer: bool,
    ) -> Result<(LocalBeaconNode<E>, LocalExecutionNode<E>), String> {
        let beacon_node_count = self.beacon_nodes.read().unwrap().len();
        let proposer_node_count = self.proposer_nodes.read().unwrap().len();
        let count = (beacon_node_count + proposer_node_count) as u16;

        // Set config.
        let libp2p_tcp_port = BOOTNODE_PORT + count;
        let discv5_port = BOOTNODE_PORT + count;
        beacon_config.network.set_ipv4_listening_address(
            std::net::Ipv4Addr::UNSPECIFIED,
            libp2p_tcp_port,
            discv5_port,
            QUIC_PORT + count,
        );
        beacon_config.network.enr_udp4_port = Some(discv5_port.try_into().unwrap());
        beacon_config.network.enr_tcp4_port = Some(libp2p_tcp_port.try_into().unwrap());
        beacon_config.network.discv5_config.table_filter = |_| true;
        beacon_config.network.proposer_only = is_proposer;

        mock_execution_config.server_config.listen_port = EXECUTION_PORT + count;

        // Construct execution node.
        let execution_node = LocalExecutionNode::new(
            self.context.service_context(format!("node_{}_el", count)),
            mock_execution_config,
        );

        // Pair the beacon node and execution node.
        beacon_config.execution_layer = Some(execution_layer::Config {
            execution_endpoint: Some(
                Eth2SensitiveUrl::parse(&execution_node.server.url()).unwrap(),
            ),
            default_datadir: execution_node.datadir.path().to_path_buf(),
            secret_file: Some(execution_node.datadir.path().join("jwt.hex")),
            ..Default::default()
        });

        // Construct beacon node using the config,
        let beacon_node = LocalBeaconNode::production(
            self.context.service_context(format!("node_{}", count)),
            beacon_config,
        )
        .await?;

        Ok((beacon_node, execution_node))
    }

    pub fn remote_nodes(&self) -> Result<Vec<BeaconNodeHttpClient>, String> {
        let beacon_nodes = self.beacon_nodes.read().expect("Failed to get read lock");
        let proposer_nodes = self.proposer_nodes.read().expect("Failed to get read lock");

        beacon_nodes
            .iter()
            .chain(proposer_nodes.iter())
            .map(|beacon_node| beacon_node.remote_node())
            .collect()
    }

    pub async fn duration_to_genesis(&self) -> Result<Duration, &'static str> {
        let nodes = self.remote_nodes().expect("Failed to get remote nodes");
        let bootnode = nodes.first().expect("Should contain bootnode");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let genesis_time = Duration::from_secs(
            bootnode
                .get_beacon_genesis()
                .await
                .unwrap()
                .data
                .genesis_time,
        );
        genesis_time.checked_sub(now).ok_or(
            "The genesis time has already passed since all nodes started. The node startup time \
            may have regressed, and the current `GENESIS_DELAY` is no longer sufficient.",
        )
    }
}
