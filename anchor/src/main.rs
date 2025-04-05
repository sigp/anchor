use clap::Parser;
use tracing::{error, info};

mod environment;
use client::{cli::LoggingFlags, config, Client, Node};
use environment::Environment;
use keygen::Keygen;
use keysplit::Keysplit;
use logging::{filter_dependency_log, init_file_logging, LoggerConfig};
use task_executor::ShutdownReason;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    filter::FilterFn,
    fmt::self,
    prelude::*,
    EnvFilter,
};
use types::EthSpecId;

#[derive(Parser, Clone, Debug)]
struct Cli {
    #[clap(flatten)]
    pub logging_flags: LoggingFlags,

    #[clap(subcommand)]
    pub subcommand: AnchorSubcommands,
}

#[derive(Parser, Clone, Debug)]
pub enum AnchorSubcommands {
    Node(Box<Node>),
    Keysplit(Keysplit),
    Keygen(Keygen),
}

fn main() {
    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    let cli = Cli::parse();

    let _guard = enable_logging(&cli.logging_flags);

    // Construct the task executor and exit signals
    let environment = Environment::default();

    match cli.subcommand {
        AnchorSubcommands::Node(node) => start_anchor(*node, environment),
        AnchorSubcommands::Keysplit(keygen) => {
            if let Err(e) = keysplit::run_keysplitter(keygen) {
                error!("Keysplit error: {:?}", e);
            }
        }
        AnchorSubcommands::Keygen(keygen) => {
            if let Err(e) = keygen::run_keygen(keygen) {
                error!("Keygen error: {:?}", e);
            }
        }
    }
}

fn start_anchor(anchor_config: Node, mut environment: Environment) {
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

fn enable_logging(logging_flags: &LoggingFlags) -> WorkerGuard {
    let cli = logging_flags.clone();
    let filter_level: Level = cli.debug_level.into();

    let logger_config = LoggerConfig {
        path: cli.logfile_dir.clone(),
        debug_level: filter_level,
        max_log_size: cli.logfile_max_size,
        max_log_number: cli.logfile_max_number,
        compression: cli.logfile_compression,
    };

    let dependency_log_filter =
        FilterFn::new(filter_dependency_log as fn(&tracing::Metadata<'_>) -> bool);

    let file_logging_layer = init_file_logging(logger_config.clone());

    let mut logging_layers = Vec::new();

    logging_layers.push(
        fmt::layer()
            .with_filter(
                EnvFilter::builder()
                        .with_default_directive(filter_level.into())
                        .from_env_lossy(),
            )
            .with_filter(dependency_log_filter.clone())
            .boxed(),
    );

    if let Some(ref file_logging_layer) = file_logging_layer {
        logging_layers.push(
            fmt::layer()
                .with_writer(file_logging_layer.non_blocking_writer.clone())
                .with_filter(
                    EnvFilter::builder()
                        .with_default_directive(filter_level.into())
                        .from_env_lossy(),
                )
                .with_filter(dependency_log_filter)
                .boxed(),
        );
    }

    let logging_result = tracing_subscriber::registry()
        .with(logging_layers)
        .try_init();

    if let Err(e) = logging_result {
        eprintln!("Failed to initialize logger: {e}");
    }

    file_logging_layer.unwrap().guard
}
