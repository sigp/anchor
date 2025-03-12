use crate::local_network::{SsvNetworkParams, EXECUTION_PORT};
use clap::Parser;
use client::config::Config;
use client::Node;
use kzg::trusted_setup::get_trusted_setup;
use node_test_rig::{
    eth2::{types::ChainSpec, types::EthSpec, SensitiveUrl},
    testing_client_config, ClientConfig, ClientGenesis, MockExecutionConfig, MockServerConfig,
    ValidatorFiles,
};
use rayon::prelude::*;
use std::net::Ipv4Addr;

// Generate the directories and keystores required for the validator clients.
pub fn generate_validators(node_count: usize, validators_per_node: usize) -> Vec<ValidatorFiles> {
    (0..node_count)
        .into_par_iter()
        .map(|i| {
            println!(
                "Generating keystores for validator {} of {}",
                i + 1,
                node_count
            );

            let indices =
                (i * validators_per_node..(i + 1) * validators_per_node).collect::<Vec<_>>();
            ValidatorFiles::with_keystores(&indices).unwrap()
        })
        .collect::<Vec<_>>()
}

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
    beacon_config.network.target_peers =
        network_params.num_nodes + network_params.num_proposers + network_params.extra_nodes - 1;
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
    // default node config
    let node = Node::parse_from::<Vec<String>, String>(vec![]);
    client::config::from_cli(&node).unwrap()
}
