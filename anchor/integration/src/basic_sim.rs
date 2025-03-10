use crate::checks::*;
use crate::local_network::{SsvLocalNetwork, SsvNetworkParams};
use crate::util::generate_validators;
use clap::ArgMatches;
use environment::tracing_common;
use logging::MetricsLayer;
use node_test_rig::{
    environment::{EnvironmentBuilder, LoggerConfig},
    testing_validator_config, ApiTopic,
};
use tokio::time::sleep;
use tracing::{error, info};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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
        let _spec = (*env.eth2_config.spec).clone();
        let context = env.core_context();

        // Setup a future that will perform all simulation checks on the network
        let main_future = async {
            // Create the local_network
            let (network, beacon_config, execution_config) = Box::pin(
                SsvLocalNetwork::create_local_network(SsvNetworkParams::default(), context.clone()),
            )
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

            // Add operator nodes to the newtork
            for _ in 0..validator_files.len() {
                // for each validator, spawn committee size operators
            }

            // Add validators to the network
            let executor = context.executor.clone();
            for (i, files) in validator_files.into_iter().enumerate() {
                let network_1 = network.clone();
                executor.spawn(
                    async move {
                        let mut validator_config = testing_validator_config();
                        validator_config.validator_store.fee_recipient =
                            Some(SUGGESTED_FEE_RECIPIENT.into());

                        // Enable broadcast on every 4th node.
                        if i % 4 == 0 {
                            validator_config.broadcast_topics = ApiTopic::all();
                            let beacon_nodes = vec![i, (i + 1) % node_count];
                            network_1
                                .add_validator_client_with_fallbacks(
                                    validator_config,
                                    i,
                                    beacon_nodes,
                                    files,
                                )
                                .await
                        } else {
                            network_1
                                .add_validator_client(validator_config, i, files)
                                .await
                        }
                        .expect("should add validator");
                    },
                    "vc",
                );
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
