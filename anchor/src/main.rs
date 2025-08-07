use clap::Parser;
use client::{Client, Node, config};
use environment::Environment;
use global_config::{GlobalConfig, GlobalFlags};
use keygen::Keygen;
use keysplit::Keysplit;
use logging::{
    CountLayer, FileLoggingFlags, create_libp2p_discv5_tracing_layer, init_file_logging,
    utils::build_workspace_filter,
};
use task_executor::ShutdownReason;
use tracing::{Level, error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};
use types::EthSpecId;

mod environment;

#[derive(Parser, Clone, Debug)]
struct Cli {
    #[clap(flatten)]
    pub global_flags: GlobalFlags,

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
        // `set_var` is marked unsafe because it is unsafe to use if there are multiple threads
        // reading or writing from the environment. We are at the very beginning of execution and
        // have not spun up any threads or the tokio runtime, so it is safe to use.
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    let cli = Cli::parse();

    let global_config = match GlobalConfig::try_from(&cli.global_flags) {
        Ok(global_config) => global_config,
        Err(err) => {
            eprintln!("Failed to create config from CLI params: {err}");
            return;
        }
    };

    let file_logging_flags = if let AnchorSubcommands::Node(node) = &cli.subcommand {
        Some(&node.logging_flags)
    } else {
        None
    };

    let _guards = match enable_logging(file_logging_flags, &global_config) {
        Ok(guards) => guards,
        Err(err) => {
            eprintln!("Failed to initialize logging: {err}");
            return;
        }
    };

    // Construct the task executor and exit signals
    let environment = Environment::default();

    match cli.subcommand {
        AnchorSubcommands::Node(node) => start_anchor(&node, global_config, environment),
        AnchorSubcommands::Keysplit(keysplit) => {
            if let Err(e) = keysplit::run_keysplitter(keysplit, global_config) {
                error!("Keysplit error: {:?}", e);
            }
        }
        AnchorSubcommands::Keygen(keygen) => {
            if let Err(e) = keygen::run_keygen(keygen, &global_config.data_dir) {
                error!("Keygen error: {:?}", e);
            }
        }
    }
}

fn start_anchor(anchor_config: &Node, global_config: GlobalConfig, mut environment: Environment) {
    // Build the client config
    let mut config = match config::from_cli(anchor_config, global_config) {
        Ok(config) => config,
        Err(e) => {
            tracing_subscriber::fmt().init();
            error!(e, "Unable to initialize configuration");
            return;
        }
    };

    config.network.domain_type = config.global_config.ssv_network.ssv_domain_type.clone();

    // Build the core task executor
    let core_executor = environment.executor();

    // The clone's here simply copy the Arc of the runtime. We pass these through the main
    // execution task
    let anchor_executor = core_executor.clone();
    let shutdown_executor = core_executor.clone();

    let eth_spec_id = match config.global_config.ssv_network.eth2_network.eth_spec_id() {
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

pub fn enable_logging(
    file_logging_flags: Option<&FileLoggingFlags>,
    global_config: &GlobalConfig,
) -> Result<Vec<WorkerGuard>, String> {
    let mut logging_layers = Vec::new();
    let mut guards = Vec::new();

    let workspace_filter = match build_workspace_filter() {
        Ok(filter) => filter,
        Err(e) => {
            return Err(format!("Unable to build workspace filter: {e}"));
        }
    };

    logging_layers.push(
        fmt::layer()
            .with_filter(
                EnvFilter::builder()
                    .with_default_directive(global_config.debug_level.into())
                    .from_env_lossy(),
            )
            .with_filter(workspace_filter.clone())
            .boxed(),
    );

    if let Some(file_logging_flags) = file_logging_flags {
        let logs_dir = file_logging_flags
            .logfile_dir
            .clone()
            .unwrap_or_else(|| global_config.data_dir.default_logs_dir());

        let filter_level: Level = file_logging_flags.logfile_debug_level;

        let libp2p_discv5_layer = create_libp2p_discv5_tracing_layer(
            Some(logs_dir.clone()),
            file_logging_flags.logfile_max_size,
        );
        let file_logging_layer = init_file_logging(&logs_dir, file_logging_flags.clone());

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

        if let Some(file_logging_layer) = file_logging_layer {
            guards.push(file_logging_layer.guard);
            logging_layers.push(
                fmt::layer()
                    .with_writer(file_logging_layer.non_blocking_writer)
                    .with_ansi(file_logging_flags.logfile_color)
                    .with_filter(
                        EnvFilter::builder()
                            .with_default_directive(filter_level.into())
                            .from_env_lossy(),
                    )
                    .with_filter(workspace_filter.clone())
                    .boxed(),
            );
        }
    }

    // Add the CountLayer
    logging_layers.push(CountLayer.boxed());

    tracing_subscriber::registry()
        .with(logging_layers)
        .try_init()
        .map_err(|e| format!("Failed to initialize logging: {e}"))?;

    Ok(guards)
}
