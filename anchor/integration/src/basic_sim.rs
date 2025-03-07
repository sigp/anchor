use crate::local_network::SsvLocalNetwork;
use node_test_rig::{
    environment::{EnvironmentBuilder, LoggerConfig},
    testing_validator_config, ApiTopic, ValidatorFiles,
};

pub struct BasicSim {}

impl BasicSim {
    pub fn run() -> Result<(), String> {
        // Generate the directories and keystores required for the validator clients.
        let validator_files = ValidatorFiles::with_keystores(&[1]).unwrap();

        let mut env = EnvironmentBuilder::minimal()
            .initialize_logger(LoggerConfig {
                path: None,
                debug_level: "debug-level".to_string(),
                logfile_debug_level: "debug-level".to_string(),
                log_format: None,
                logfile_format: None,
                log_color: false,
                disable_log_timestamp: false,
                max_log_size: 0,
                max_log_number: 0,
                compression: false,
                is_restricted: true,
                sse_logging: false,
            })?
            .multi_threaded_tokio_runtime()?
            .build()?;


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
