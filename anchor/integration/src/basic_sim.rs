use crate::checks::*;
use crate::local_network::{SsvLocalNetwork, SsvNetworkParams};
use crate::mock_websocket::MockServer;
use crate::util::parse_cli;
use clap::ArgMatches;
use environment::tracing_common;
use node_test_rig::{
    environment::{EnvironmentBuilder, LoggerConfig},
    eth2::types::Epoch,
};
use std::cmp::max;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::time::sleep;
use tracing::info;

const GENESIS_DELAY: u64 = 32;
pub const ALTAIR_FORK_EPOCH: u64 = 0;
pub const BELLATRIX_FORK_EPOCH: u64 = 0;
pub const CAPELLA_FORK_EPOCH: u64 = 1;
pub const DENEB_FORK_EPOCH: u64 = 2;

pub struct SimConfig {
    pub node_count: usize,
    pub proposer_nodes: usize,
    pub validators_per_node: usize,
    pub speed_up_factor: u64,
    pub log_level: String,
    pub committee_size: usize,
    pub continue_after_checks: bool,
}

// Validators registered to the operators in the SSV database
static REGISTERED_VALIDATORS: LazyLock<Vec<String>> = LazyLock::new(|| {
    vec![
        String::from("0x8de7ec501d574152f52a962bf588573df2fc3563fd0c6077651208ed20f24f3d8572425706b343117b48bdca56808416"),
        String::from("0x95833097520df43a5cb013e97f80041a7a0b7d84a4ec79e2f16baeeb6edfbcf62ede97becfde73883831bb65e1415dc0"),
        String::from("0x966c488d807b3208bb1b10a1af422bac8d363c8015cda4e24d214549ced019cd3dd575545dd887461cae3f70d95cb061"),
        String::from("0xb3faeebfbebd085b9123ae0e09af9cd15d3b1db6a25f3e82d8b48b68e53522b41b342a3a3c8b008897df356048862d98")
    ]
});

pub struct BasicSim {}

impl BasicSim {
    pub fn run(matches: &ArgMatches) -> Result<(), String> {
        let sim_config = parse_cli(matches);
        info!("Basic Simulator:");
        info!(" nodes: {}", sim_config.node_count);
        info!(" proposer-nodes: {}", sim_config.proposer_nodes);
        info!(" validators-per-node: {}", sim_config.validators_per_node);
        info!(" speed-up-factor: {}", sim_config.speed_up_factor);
        info!(
            " continue-after-checks: {}",
            sim_config.continue_after_checks
        );

        let (
            env_builder,
            _filter_layer,
            _,
            _file_logging_layer,
            _stdout_logging_layer,
            _,
            _logger_config,
            _,
        ) = tracing_common::construct_logger(
            LoggerConfig {
                path: None,
                debug_level: tracing_common::parse_level(&sim_config.log_level.clone()),
                logfile_debug_level: tracing_common::parse_level(&sim_config.log_level.clone()),
                log_format: None,
                logfile_format: None,
                log_color: true,
                logfile_color: true,
                disable_log_timestamp: false,
                max_log_size: 0,
                max_log_number: 0,
                compression: false,
                is_restricted: true,
                sse_logging: false,
                extra_info: false,
            },
            matches,
            EnvironmentBuilder::mainnet(),
        );

        let mut env = env_builder.multi_threaded_tokio_runtime()?.build()?;
        let mut spec = (*env.eth2_config.spec).clone();
        let total_validator_count = sim_config.validators_per_node * sim_config.node_count;
        let genesis_delay = GENESIS_DELAY;
        spec.seconds_per_slot /= sim_config.speed_up_factor;
        spec.seconds_per_slot = max(1, spec.seconds_per_slot);
        spec.genesis_delay = genesis_delay;
        spec.min_genesis_time = 0;
        spec.min_genesis_active_validator_count = total_validator_count as u64;
        spec.altair_fork_epoch = Some(Epoch::new(ALTAIR_FORK_EPOCH));
        spec.bellatrix_fork_epoch = Some(Epoch::new(BELLATRIX_FORK_EPOCH));
        spec.capella_fork_epoch = Some(Epoch::new(CAPELLA_FORK_EPOCH));
        spec.deneb_fork_epoch = Some(Epoch::new(DENEB_FORK_EPOCH));
        env.eth2_config.spec = Arc::new(spec);

        // Start the mock server
        let server = env
            .runtime()
            .block_on(async { MockServer::start().await })?;

        info!("Mock server available at: {}", server.url);

        // Setup a future that will perform all simulation checks on the network
        let main_future = async {
            // Create the local_network
            let (network, beacon_config, execution_config, anchor_config) =
                Box::pin(SsvLocalNetwork::create_local_network(
                    SsvNetworkParams {
                        num_validators: sim_config.validators_per_node * sim_config.node_count,
                        num_nodes: sim_config.node_count,
                        num_proposers: sim_config.proposer_nodes,
                        genesis_delay: GENESIS_DELAY,
                    },
                    env.core_context(),
                ))
                .await?;

            // Add beacon + execution node to the network
            for _ in 0..sim_config.node_count {
                network
                    .add_beacon_node(beacon_config.clone(), execution_config.clone(), false)
                    .await?;
            }

            // Register the validators...
            // todo!()

            // Add operator nodes to the network
            for index in 0..(sim_config.committee_size) {
                network
                    .add_anchor_node(index, anchor_config.clone(), server.url.clone())
                    .await?;
            }

            // Set all payloads as valid. This effectively assumes the EL is infalliable.
            network
                .execution_nodes
                .write()
                .expect("Failed to get write lock")
                .iter()
                .for_each(|node| {
                    node.server.all_payloads_valid();
                });

            // Sleep until we hit genesis
            let duration_to_genesis = network.duration_to_genesis().await?;
            info!("Duration to genesis: {}", duration_to_genesis.as_secs());
            sleep(duration_to_genesis).await;

            // Run all checks and verify their success
            let test1 = futures::join!(mock_verify());
            test1.0?;

            futures::future::pending::<()>().await;

            Ok::<(), String>(())
        };

        env.runtime().block_on(main_future).unwrap();
        env.fire_signal();
        env.shutdown_on_idle();

        Ok(())
    }
}
