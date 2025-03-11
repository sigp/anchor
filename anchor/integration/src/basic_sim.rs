use crate::checks::*;
use crate::local_network::{SsvLocalNetwork, SsvNetworkParams};
use crate::util::generate_validators;
use clap::ArgMatches;
use environment::tracing_common;
use logging::MetricsLayer;
use node_test_rig::{
    environment::{EnvironmentBuilder, LoggerConfig},
    eth2::types::{Epoch, EthSpec, MinimalEthSpec},
    testing_validator_config, ApiTopic,
};
use std::cmp::max;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, info};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const GENESIS_DELAY: u64 = 32;
const END_EPOCH: u64 = 16;
const ALTAIR_FORK_EPOCH: u64 = 0;
const BELLATRIX_FORK_EPOCH: u64 = 0;
const CAPELLA_FORK_EPOCH: u64 = 1;
const DENEB_FORK_EPOCH: u64 = 2;
const SUGGESTED_FEE_RECIPIENT: [u8; 20] =
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

pub struct BasicSim {}

impl BasicSim {
    pub fn run(matches: &ArgMatches) -> Result<(), String> {
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

        info!("Basic Simulator:");
        info!(" nodes: {}", node_count);
        info!(" proposer-nodes: {}", proposer_nodes);
        info!(" validators-per-node: {}", validators_per_node);
        info!(" speed-up-factor: {}", speed_up_factor);
        info!(" continue-after-checks: {}", continue_after_checks);

        // Generate the directories and keystores required for the validator clients.
        let validator_files = generate_validators(node_count, validators_per_node);

        let (
            env_builder,
            filter_layer,
            _,
            file_logging_layer,
            stdout_logging_layer,
            _,
            logger_config,
            _,
        ) = tracing_common::construct_logger(
            LoggerConfig {
                path: None,
                debug_level: tracing_common::parse_level(&log_level.clone()),
                logfile_debug_level: tracing_common::parse_level(&log_level.clone()),
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

        if let Err(e) = tracing_subscriber::registry()
            .with(filter_layer)
            .with(file_logging_layer.with_filter(logger_config.logfile_debug_level))
            .with(stdout_logging_layer.with_filter(logger_config.debug_level))
            .with(MetricsLayer)
            .try_init()
        {
            error!("Failed to initialize dependency logging: {e}");
        }

        let mut env = env_builder.multi_threaded_tokio_runtime()?.build()?;
        let mut spec = (*env.eth2_config.spec).clone();
        let total_validator_count = validators_per_node * node_count;
        let genesis_delay = GENESIS_DELAY;

        // Convenience variables. Update these values when adding a newer fork.
        let _latest_fork_version = spec.deneb_fork_version;
        let _latest_fork_start_epoch = DENEB_FORK_EPOCH;

        spec.seconds_per_slot /= speed_up_factor;
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
            let (network, beacon_config, execution_config) =
                Box::pin(SsvLocalNetwork::create_local_network(
                    SsvNetworkParams {
                        num_operators: validators_per_node * node_count * committee_size,
                        num_validators: validators_per_node * node_count,
                        committee_size,
                        num_nodes: node_count,
                        extra_nodes: 1,
                        num_proposers: proposer_nodes,
                        genesis_delay: GENESIS_DELAY,
                    },
                    context.clone(),
                ))
                .await?;

            // Add nodes to the network.
            for _ in 0..node_count {
                network
                    .add_beacon_node(beacon_config.clone(), execution_config.clone(), false)
                    .await?;
            }

            // Add proposer nodes to the network
            for _ in 0..proposer_nodes {
                network
                    .add_beacon_node(beacon_config.clone(), execution_config.clone(), true)
                    .await?;
            }

            // Add operator nodes to the network
            for index in 0..(committee_size) {
                network.add_operator_node(index).await?;
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

            Ok::<(), String>(())
        };

        env.runtime().block_on(main_future).unwrap();
        env.fire_signal();
        env.shutdown_on_idle();

        Ok(())
    }
}
