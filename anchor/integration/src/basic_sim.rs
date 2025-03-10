//use crate::local_network::SsvLocalNetwork;
use environment::tracing_common;
use logging::MetricsLayer;
use node_test_rig::{
    environment::{EnvironmentBuilder, LoggerConfig},
    testing_validator_config, ApiTopic, ValidatorFiles,
};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use clap::ArgMatches;

pub struct BasicSim {}

impl BasicSim {
    pub fn run(matches: &ArgMatches) -> Result<(), String> {
        // Generate the directories and keystores required for the validator clients.
        let validator_files = ValidatorFiles::with_keystores(&[1]).unwrap();

        let log_level = "debug-level".to_string();

        let (
            env_builder,
            filter_layer,
            _libp2p_discv5_layer,
            file_logging_layer,
            stdout_logging_layer,
            _sse_logging_layer_opt,
            logger_config,
            _dependency_log_filter,
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
            eprintln!("Failed to initialize dependency logging: {e}");
        }

        let mut env = env_builder.multi_threaded_tokio_runtime()?.build()?;

        let mut spec = (*env.eth2_config.spec).clone();

        // Setup a future that will perform all simulation checks on the network
        let main_future = async {
            // Create the local_network
            // todo!()

            // Add beacon nodes to the network
            // todo!()

            // Add validator to the network
            // todo!()

            // Add the operators to the network
            // todo!()

            // let (test1, test2) = futures::join!(
            //      todo!() all of the checks go here
            // );

            //test1?
            //test2?

            Ok::<(), String>(())
        };

        env.runtime().block_on(main_future).unwrap();
        env.fire_signal();
        env.shutdown_on_idle();

        Ok(())
    }
}
