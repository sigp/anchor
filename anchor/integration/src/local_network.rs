use kzg::trusted_setup::get_trusted_setup;
use node_test_rig::{
    environment::RuntimeContext,
    eth2::{types::ChainSpec, types::EthSpec, types::StateId, BeaconNodeHttpClient},
    testing_client_config, ClientConfig, ClientGenesis, LocalBeaconNode, LocalExecutionNode,
    LocalValidatorClient, MockExecutionConfig, MockServerConfig, ValidatorConfig, ValidatorFiles,
};
use sensitive_url::SensitiveUrl;
use std::net::Ipv4Addr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BOOTNODE_PORT: u16 = 42424;
const QUIC_PORT: u16 = 43424;
pub const EXECUTION_PORT: u16 = 4000;
pub const TERMINAL_BLOCK: u64 = 0;

pub struct SsvLocalNetwork<E: EthSpec> {
    pub inner: Arc<Inner<E>>,
}

pub struct Inner<E: EthSpec> {
    pub context: RuntimeContext<E>,
    pub validators: RwLock<Vec<LocalBeaconNode<E>>>,
    pub beacon_nodes: RwLock<Vec<LocalBeaconNode<E>>>,
}

pub struct SsvNetworkParams {
    pub num_operators: usize,
    pub num_validators: usize,
    pub num_nodes: usize,
    pub num_proposers: usize,
    pub extra_nodes: usize,
    pub genesis_delay: u64,
}

impl SsvNetworkParams {
    // Default network state based on the hardcoded db
    pub fn default() -> Self {
        Self {
            num_operators: 4,
            num_validators: 1,
            num_nodes: 4,
            num_proposers: 1,
            extra_nodes: 0,
            genesis_delay: 10,
        }
    }
}

impl<E: EthSpec> SsvLocalNetwork<E> {
    pub async fn create_local_network(
        network_params: SsvNetworkParams,
        context: RuntimeContext<E>,
    ) -> Result<(SsvLocalNetwork<E>, ClientConfig, MockExecutionConfig), String> {
        let genesis_time: u64 = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "should get system time")?
            + Duration::from_secs(network_params.genesis_delay))
        .as_secs();

        let beacon_config = default_client_config(network_params, genesis_time);
        let execution_config =
            default_mock_execution_config::<E>(&context.eth2_config().spec, genesis_time);

        let network = Self {
            inner: Arc::new(Inner {
                context,
                validators: RwLock::new(Vec::new()),
                beacon_nodes: RwLock::new(Vec::new()),
            }),
        };

        Ok((network, beacon_config, execution_config))
    }
}

fn default_mock_execution_config<E: EthSpec>(
    spec: &ChainSpec,
    genesis_time: u64,
) -> MockExecutionConfig {
    let mut mock_execution_config = MockExecutionConfig {
        server_config: MockServerConfig {
            listen_port: EXECUTION_PORT,
            ..Default::default()
        },
        ..Default::default()
    };

    if let Some(capella_fork_epoch) = spec.capella_fork_epoch {
        mock_execution_config.shanghai_time = Some(
            genesis_time
                + spec.seconds_per_slot * E::slots_per_epoch() * capella_fork_epoch.as_u64(),
        )
    }
    if let Some(deneb_fork_epoch) = spec.deneb_fork_epoch {
        mock_execution_config.cancun_time = Some(
            genesis_time + spec.seconds_per_slot * E::slots_per_epoch() * deneb_fork_epoch.as_u64(),
        )
    }
    if let Some(electra_fork_epoch) = spec.electra_fork_epoch {
        mock_execution_config.prague_time = Some(
            genesis_time
                + spec.seconds_per_slot * E::slots_per_epoch() * electra_fork_epoch.as_u64(),
        )
    }

    mock_execution_config
}

fn default_client_config(network_params: SsvNetworkParams, genesis_time: u64) -> ClientConfig {
    let mut beacon_config = testing_client_config();

    beacon_config.genesis = ClientGenesis::InteropMerge {
        validator_count: network_params.num_validators,
        genesis_time,
    };
    beacon_config.network.target_peers =
        network_params.num_nodes + network_params.num_proposers + network_params.extra_nodes - 1;
    beacon_config.network.enr_address = (Some(Ipv4Addr::LOCALHOST), None);
    beacon_config.network.enable_light_client_server = true;
    beacon_config.network.discv5_config.enable_packet_filter = false;
    beacon_config.chain.enable_light_client_server = true;
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
