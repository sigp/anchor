use crate::checks::*;
use crate::local_network::{SsvLocalNetwork, SsvNetworkParams};
use crate::util::parse_cli;
use clap::ArgMatches;
use environment::tracing_common;
use node_test_rig::{
    environment::{EnvironmentBuilder, LoggerConfig},
    eth2::types::{Epoch, EthSpec, MinimalEthSpec},
};
use std::cmp::max;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::info;

const GENESIS_DELAY: u64 = 32;
const ALTAIR_FORK_EPOCH: u64 = 0;
const BELLATRIX_FORK_EPOCH: u64 = 0;
const CAPELLA_FORK_EPOCH: u64 = 1;
const DENEB_FORK_EPOCH: u64 = 2;

pub struct SimConfig {
    pub node_count: usize,
    pub proposer_nodes: usize,
    pub validators_per_node: usize,
    pub speed_up_factor: u64,
    pub log_level: String,
    pub committee_size: usize,
    pub continue_after_checks: bool

}

pub struct BasicSim {}

impl BasicSim {
    pub fn run(matches: &ArgMatches) -> Result<(), String> {
        let sim_config = parse_cli(matches);
        info!("Basic Simulator:");
        println!(" nodes: {}", sim_config.node_count);
        println!(" proposer-nodes: {}", sim_config.proposer_nodes);
        println!(" validators-per-node: {}", sim_config.validators_per_node);
        println!(" speed-up-factor: {}", sim_config.speed_up_factor);
        println!(" continue-after-checks: {}", sim_config.continue_after_checks);

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
            EnvironmentBuilder::minimal(),
        );

        let mut env = env_builder.multi_threaded_tokio_runtime()?.build()?;
        let mut spec = (*env.eth2_config.spec).clone();
        let total_validator_count = sim_config.validators_per_node * sim_config.node_count;
        let genesis_delay = GENESIS_DELAY;

        // Convenience variables. Update these values when adding a newer fork.
        let _latest_fork_version = spec.deneb_fork_version;
        let _latest_fork_start_epoch = DENEB_FORK_EPOCH;

        spec.seconds_per_slot /= sim_config.speed_up_factor;
        spec.seconds_per_slot = max(1, spec.seconds_per_slot);
        spec.genesis_delay = genesis_delay;
        spec.min_genesis_time = 0;
        spec.min_genesis_active_validator_count = total_validator_count as u64;
        spec.altair_fork_epoch = Some(Epoch::new(ALTAIR_FORK_EPOCH));
        spec.bellatrix_fork_epoch = Some(Epoch::new(BELLATRIX_FORK_EPOCH));
        spec.capella_fork_epoch = Some(Epoch::new(CAPELLA_FORK_EPOCH));
        spec.deneb_fork_epoch = Some(Epoch::new(DENEB_FORK_EPOCH));
        //spec.electra_fork_epoch = Some(Epoch::new(ELECTRA_FORK_EPOCH));
        //spec.fulu_fork_epoch = Some(Epoch::new(FULU_FORK_EPOCH));
        let spec = Arc::new(spec);
        env.eth2_config.spec = spec.clone();

        let _slot_duration = Duration::from_secs(spec.seconds_per_slot);
        let _slots_per_epoch = MinimalEthSpec::slots_per_epoch();
        let _initial_validator_count = spec.min_genesis_active_validator_count as usize;
        let context = env.core_context();

        // Setup a future that will perform all simulation checks on the network
        let main_future = async {
            // Create the local_network
            let (network, beacon_config, execution_config, anchor_config) =
                Box::pin(SsvLocalNetwork::create_local_network(
                    SsvNetworkParams {
                        num_operators: sim_config.validators_per_node * sim_config.node_count * sim_config.committee_size,
                        num_validators: sim_config.validators_per_node * sim_config.node_count,
                        committee_size: sim_config.committee_size,
                        num_nodes: sim_config.node_count,
                        extra_nodes: 1,
                        num_proposers: sim_config.proposer_nodes,
                        genesis_delay: GENESIS_DELAY,
                    },
                    context.clone(),
                ))
                .await?;

            // Add nodes to the network.
            for _ in 0..sim_config.node_count {
                network
                    .add_beacon_node(beacon_config.clone(), execution_config.clone(), false)
                    .await?;
            }

            // Add proposer nodes to the network
            for _ in 0..sim_config.proposer_nodes {
                network
                    .add_beacon_node(beacon_config.clone(), execution_config.clone(), true)
                    .await?;
            }

            // Add operator nodes to the network
            for index in 0..(sim_config.committee_size) {
                network
                    .add_anchor_node(index, anchor_config.clone())
                    .await?;
            }

            // Set all payloads as valid. This effectively assumes the EL is infalliable.
            network.execution_nodes.write().iter().for_each(|_node| {
                //node.server.all_payloads_valid();
            });

            // Sleep until we hit genesis
            let duration_to_genesis = network.duration_to_genesis().await?;
            println!("Duration to genesis: {}", duration_to_genesis.as_secs());
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
