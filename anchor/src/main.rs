use std::path::PathBuf;

use clap::Parser;
use tracing::{error, info};

mod environment;
use client::{
    config::{self, DEFAULT_ROOT_DIR},
    Client, Node,
};
use environment::Environment;
use keygen::Keygen;
use keysplit::Keysplit;
use logging::{
    create_libp2p_discv5_tracing_layer, init_file_logging, utils::build_workspace_filter,
    Libp2pDiscv5TracingLayer, LoggerConfig, LoggingLayer,
};
use task_executor::ShutdownReason;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use types::EthSpecId;

#[derive(Parser, Clone, Debug)]
struct Cli {
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

    let (guard_opt, _libp2p_discv5_layer) = match cli.subcommand {
        AnchorSubcommands::Node(ref node) => enable_logging(node),
        _ => {
            tracing_subscriber::fmt().init();
            (None, None)
        }
    };
    let _guard = guard_opt.unwrap_or_else(|| tracing_appender::non_blocking(std::io::sink()).1);

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
            tracing_subscriber::fmt().init();
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

fn enable_logging(anchor_config: &Node) -> (Option<WorkerGuard>, Option<Libp2pDiscv5TracingLayer>) {
    let config = match config::from_cli(anchor_config) {
        Ok(config) => config,
        Err(_) => {
            return (None, None);
        }
    };

    let default_logs_dir = if let Some(datadir) = &anchor_config.datadir {
        datadir.join("logs")
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(DEFAULT_ROOT_DIR)
            .join(
                config
                    .ssv_network
                    .eth2_network
                    .config
                    .config_name
                    .as_deref()
                    .unwrap_or("custom"),
            )
            .join("logs")
    };

    let cli = anchor_config.logging_flags.clone();
    let filter_level: Level = cli.clone().logfile_debug_level.into();

    let logger_config = LoggerConfig {
        path: if cli.logfile_dir.is_some() {
            cli.logfile_dir.clone()
        } else {
            Some(default_logs_dir.clone())
        },
        debug_level: filter_level,
        max_log_size: cli.logfile_max_size,
        max_log_number: cli.logfile_max_number,
        compression: cli.logfile_compression,
    };

    let workspace_filter = match build_workspace_filter() {
        Ok(filter) => filter,
        Err(e) => {
            eprintln!("Unable to build workspace filter: {e}");
            return (None, None);
        }
    };

    let libp2p_discv5_layer = create_libp2p_discv5_tracing_layer(
        logger_config.clone().path,
        logger_config.clone().max_log_size,
    );
    let file_logging_layer = init_file_logging(default_logs_dir, logger_config.clone());

    let mut logging_layers = Vec::new();

    logging_layers.push(
        fmt::layer()
            .with_filter(
                EnvFilter::builder()
                    .with_default_directive(Level::from(cli.debug_level).into())
                    .from_env_lossy(),
            )
            .with_filter(workspace_filter.clone())
            .boxed(),
    );

    if let Some(libp2p_discv5_layer) = libp2p_discv5_layer {
        logging_layers.push(
            libp2p_discv5_layer
                .with_filter(
                    EnvFilter::builder()
                        .with_default_directive(Level::DEBUG.into())
                        .from_env_lossy(),
                )
                .boxed(),
        );
    }

    if let Some(ref file_logging_layer) = file_logging_layer {
        logging_layers.push(
            fmt::layer()
                .with_writer(file_logging_layer.non_blocking_writer.clone())
                .with_filter(
                    EnvFilter::builder()
                        .with_default_directive(filter_level.into())
                        .from_env_lossy(),
                )
                .with_filter(workspace_filter.clone())
                .boxed(),
        );
    }

    let logging_result = tracing_subscriber::registry()
        .with(logging_layers)
        .try_init();

    if let Err(e) = logging_result {
        eprintln!("Failed to initialize logger: {e}");
    }

    let libp2p_discv5_layer = create_libp2p_discv5_tracing_layer(
        logger_config.clone().path,
        logger_config.clone().max_log_size,
    );
    (
        Some(
            file_logging_layer
                .unwrap_or_else(|| {
                    let (writer, guard) = tracing_appender::non_blocking(std::io::sink());
                    LoggingLayer::new(writer, guard)
                })
                .guard,
        ),
        libp2p_discv5_layer,
    )
}
