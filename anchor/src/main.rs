use clap::Parser;
use tracing::{error, info};

mod environment;
use client::{config, Anchor, Client};
use environment::Environment;
use task_executor::ShutdownReason;
use types::EthSpecId;

fn main() {
    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // Construct the logging, task executor and exit signals
    let mut environment = Environment::default();

    // Obtain the CLI and build the config
    let anchor_config: Anchor = Anchor::parse();

    // Currently the only binary is the client. We build the client config, but later this will
    // generalise to other sub commands
    // Build the client config
    let mut config = match config::from_cli(&anchor_config) {
        Ok(config) => config,
        Err(e) => {
            error!(e, "Unable to initialize configuration");
            return;
        }
    };
    config.network.domain_type = config.ssv_network.ssv_domain_type.clone();

    // Build the core task executor
    let core_executor = environment.executor();

    // The clone's here simply copy the Arc of the runtime. We pass these through the main
    // execution task
    let anchor_executor = core_executor.clone();
    let shutdown_executor = core_executor.clone();

    let eth_spec_id = match config.ssv_network.eth2_network.eth_spec_id() {
        Ok(eth_spec_id) => eth_spec_id,
        Err(e) => {
            error!(e, "Unable to get eth spec id");
            return;
        }
    };

    // Run the main task
    core_executor.spawn(
        async move {
            let result = match eth_spec_id {
                EthSpecId::Mainnet => {
                    Client::run::<types::MainnetEthSpec>(anchor_executor, config).await
                }
                #[cfg(feature = "spec-minimal")]
                EthSpecId::Minimal => {
                    Client::run::<types::MinimalEthSpec>(anchor_executor, config).await
                }
                other => Err(format!(
                    "Eth spec `{other}` is not supported by this build of Anchor",
                )),
            };
            if let Err(e) = result {
                error!(reason = e, "Failed to start Anchor");
                // Ignore the error since it always occurs during normal operation when
                // shutting down.
                let _ = shutdown_executor
                    .shutdown_sender()
                    .try_send(ShutdownReason::Failure("Failed to start Anchor"));
            }
        },
        "anchor_client",
    );

    // Block this thread until we get a ctrl-c or a task sends a shutdown signal.
    let shutdown_reason = match environment.block_until_shutdown_requested() {
        Ok(reason) => reason,
        Err(e) => {
            error!(error = ?e, "Failed to shutdown");
            return;
        }
    };
    info!(reason = ?shutdown_reason, "Shutting down...");

    environment.fire_signal();

    // Shutdown the environment once all tasks have completed.
    environment.shutdown_on_idle();

    match shutdown_reason {
        ShutdownReason::Success(_) => {}
        ShutdownReason::Failure(msg) => {
            error!(reason = msg.to_string(), "Failed to shutdown gracefully");
        }
    };
}
