use tracing::{error, info};

mod cli;
mod client;
mod config;
mod environment;
mod qbft;
mod qbft2;
mod version;
use client::SSVClient;
use environment::Environment;
use futures::TryFutureExt;
use task_executor::ShutdownReason;

fn main() {
    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // Obtain the CLI and build the config
    let cli = cli::cli_app();
    let matches = cli.get_matches();
    // Build the config
    let config = match config::from_cli(&matches) {
        Ok(config) => config,
        Err(e) => {
            error!(e, "Unable to initialize configuration");
            return;
        }
    };

    // Construct the task executor and exit signals
    let mut environment = Environment::default();
    // Build the core task executor
    let core_executor = environment.executor();

    // The clone's here simply copy the Arc of the runtime. We pass these through the main
    // execution task
    let ssv_executor = core_executor.clone();
    let shutdown_executor = core_executor.clone();

    // Run the main task
    core_executor.spawn(
        async move {
            if let Err(e) = SSVClient::new(ssv_executor, config)
                .and_then(|mut client| async move { client.run().await })
                .await
            {
                error!(reason = e, "Failed to start SSZ client");
                // Ignore the error since it always occurs during normal operation when
                // shutting down.
                let _ = shutdown_executor
                    .shutdown_sender()
                    .try_send(ShutdownReason::Failure("Failed to start SSZ client"));
            }
        },
        "ssz_client",
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
