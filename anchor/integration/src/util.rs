use crate::basic_sim::SimConfig;
use crate::basic_sim::{
    ALTAIR_FORK_EPOCH, BELLATRIX_FORK_EPOCH, CAPELLA_FORK_EPOCH, DENEB_FORK_EPOCH,
};
use crate::local_network::{SsvNetworkParams, EXECUTION_PORT};
use clap::ArgMatches;
use clap::Parser;
use client::config::Config;
use client::DebugLevel;
use client::Node;
use kzg::trusted_setup::get_trusted_setup;
use node_test_rig::{
    eth2::{types::ChainSpec, types::EthSpec, SensitiveUrl},
    testing_client_config, ClientConfig, ClientGenesis, MockExecutionConfig, MockServerConfig,
};
use serde_utils::quoted_u64::MaybeQuoted;
use ssv_network_config::SsvNetworkConfig;
use std::net::Ipv4Addr;
use types::Epoch;

pub fn default_mock_execution_config<E: EthSpec>(
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

// Create a default beaco node config
pub fn default_client_config(network_params: SsvNetworkParams, genesis_time: u64) -> ClientConfig {
    let mut beacon_config = testing_client_config();

    beacon_config.genesis = ClientGenesis::InteropMerge {
        validator_count: network_params.num_validators,
        genesis_time,
    };
    beacon_config.network.target_peers = network_params.num_nodes + network_params.num_proposers;
    beacon_config.network.enr_address = (Some(Ipv4Addr::LOCALHOST), None);
    beacon_config.network.enable_light_client_server = true;
    beacon_config.network.discv5_config.enable_packet_filter = false;
    beacon_config.chain.enable_light_client_server = true;
    beacon_config.chain.optimistic_finalized_sync = false;
    beacon_config.trusted_setup = serde_json::from_reader(get_trusted_setup().as_slice())
        .expect("Trusted setup bytes should be valid");

    let el_config = execution_layer::Config {
        execution_endpoint: Some(
            SensitiveUrl::parse(&format!("http://localhost:{}", EXECUTION_PORT)).unwrap(),
        ),
        ..Default::default()
    };
    beacon_config.execution_layer = Some(el_config);
    beacon_config
}

// Create a default anchor operator configuration
pub fn default_anchor_config() -> Config {
    let mut node = Node::parse_from::<Vec<String>, String>(vec![]);
    node.debug_level = DebugLevel::Debug;

    let mut anchor_config = client::config::from_cli(&node).unwrap();

    anchor_config.ssv_network = SsvNetworkConfig::constant("mainnet").unwrap().unwrap();
    anchor_config.skip_sync = true;

    let mut network_config = anchor_config.ssv_network.eth2_network.config.clone();
    network_config.altair_fork_epoch = Some(MaybeQuoted {
        value: Epoch::new(ALTAIR_FORK_EPOCH),
    });
    network_config.bellatrix_fork_epoch = Some(MaybeQuoted {
        value: Epoch::new(BELLATRIX_FORK_EPOCH),
    });
    network_config.capella_fork_epoch = Some(MaybeQuoted {
        value: Epoch::new(CAPELLA_FORK_EPOCH),
    });
    network_config.deneb_fork_epoch = Some(MaybeQuoted {
        value: Epoch::new(DENEB_FORK_EPOCH),
    });
    anchor_config.ssv_network.eth2_network.config = network_config;

    anchor_config
}

// Parse the cli arguments into a simulation config
pub fn parse_cli(matches: &ArgMatches) -> SimConfig {
    // Extract out confirguration options
    let node_count = matches
        .get_one::<String>("nodes")
        .expect("missing nodes default")
        .parse::<usize>()
        .expect("missing nodes default");
    let proposer_nodes = matches
        .get_one::<String>("proposer-nodes")
        .unwrap_or(&String::from("0"))
        .parse::<usize>()
        .unwrap_or(0);
    let validators_per_node = matches
        .get_one::<String>("validators-per-node")
        .expect("missing validators-per-node default")
        .parse::<usize>()
        .expect("missing validators-per-node default");
    let speed_up_factor = matches
        .get_one::<String>("speed-up-factor")
        .expect("missing speed-up-factor default")
        .parse::<u64>()
        .expect("missing speed-up-factor default");
    let log_level = matches
        .get_one::<String>("debug-level")
        .expect("missing debug-level");
    let committee_size = matches
        .get_one::<String>("committee-size")
        .expect("missing committee-size default")
        .parse::<usize>()
        .expect("committee-size must be a number");
    let continue_after_checks = matches.get_flag("continue-after-checks");
    SimConfig {
        node_count,
        proposer_nodes,
        validators_per_node,
        speed_up_factor,
        log_level: log_level.to_string(),
        committee_size,
        continue_after_checks,
    }
}
