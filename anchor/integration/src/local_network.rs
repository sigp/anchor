use types::EthSpec;
use std::sync::{RwLock, Arc};
use kzg::trusted_setup::get_trusted_setup;
use node_test_rig::{
    environment::RuntimeContext,
    eth2::{types::StateId, BeaconNodeHttpClient},
    testing_client_config, ClientConfig, ClientGenesis, LocalBeaconNode, LocalExecutionNode,
    LocalValidatorClient, MockExecutionConfig, MockServerConfig, ValidatorConfig, ValidatorFiles,
};
use sensitive_url::SensitiveUrl;
use std::net::Ipv4Addr;

const BOOTNODE_PORT: u16 = 42424;
const QUIC_PORT: u16 = 43424;
pub const EXECUTION_PORT: u16 = 4000;
pub const TERMINAL_BLOCK: u64 = 0;

fn default_client_config(network_params: SsvNetworkParams, genesis_time: u64) -> ClientConfig {
    let mut beacon_config = testing_client_config();

    //beacon_config.genesis = ClientGenesis::InteropMerge {
    //   validator_count: network_params.validator_count,
    //   genesis_time,
    //};
    //beacon_config.network.target_peers =
        //network_params.node_count + network_params.proposer_nodes + network_params.extra_nodes - 1;
    beacon_config.network.enr_address = (Some(Ipv4Addr::LOCALHOST), None);
    beacon_config.network.enable_light_client_server = true;
    beacon_config.network.discv5_config.enable_packet_filter = false;
    beacon_config.chain.enable_light_client_server = true;
    beacon_config.http_api.enable_light_client_server = true;
    beacon_config.chain.optimistic_finalized_sync = false;
    beacon_config.trusted_setup = serde_json::from_reader(get_trusted_setup().as_slice())
        .expect("Trusted setup bytes should be valid");

    /*
    let el_config = execution_layer::Config {
        execution_endpoint: Some(
            SensitiveUrl::parse(&format!("http://localhost:{}", EXECUTION_PORT)).unwrap(),
        ),
        ..Default::default()
    };
    beacon_config.execution_layer = Some(el_config);
    */
    beacon_config
}



pub struct SsvNetworkParams {
    pub num_operators: usize,
    pub num_validators: usize,
}

impl SsvNetworkParams {
    // Default network state based on the hardcoded db
    fn default() -> Self {
        Self {
            num_operators: 4,
            num_validators: 1
        }
    }
}

pub struct Inner<E: EthSpec> {
    pub validators: RwLock<Vec<LocalBeaconNode<E>>>,
    pub beacon_nodes: RwLock<Vec<LocalBeaconNode<E>>>,
}

pub struct SsvLocalNetwork<E: EthSpec> {
    pub params: SsvNetworkParams,
    pub inner: Arc<Inner<E>>

}

impl<E: EthSpec> SsvLocalNetwork<E> {
    pub fn create_local_network() -> SsvLocalNetwork<E> {
        Self {
            params: SsvNetworkParams::default(),
            inner: Arc::new(Inner {
                validators: RwLock::new(Vec::new()),
                beacon_nodes: RwLock::new(Vec::new())
            })
        }
    }

    pub fn add_execution_node() {
        todo!()
    }

    pub fn add_beacon_node() {
        todo!()
    }

    pub fn add_operator_node() {
        todo!()
    }

    pub fn add_validator_node() {
        todo!()

    }
}

