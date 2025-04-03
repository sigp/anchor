use clap::Parser;
use tracing::{error, info};

mod environment;
use client::cli::DebugLevel;
use client::{config, Client, Node};
use environment::Environment;
use keygen::Keygen;
use keysplit::Keysplit;
use logging::{filter_dependency_log, init_file_logging, LoggerConfig};
use std::path::PathBuf;
use task_executor::ShutdownReason;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::FilterFn;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;
use types::EthSpecId;

#[derive(Parser, Clone, Debug)]
struct Cli {
    #[clap(subcommand)]
    pub subcommand: AnchorSubcommands,
    
    #[arg(
        long,
        default_value_t = DebugLevel::Info,
        help = "Specifies the verbosity level used when emitting logs to the terminal")]
    pub debug_level: DebugLevel,

    #[arg(
        long,
        global = true,
        value_name = "SIZE",
        help = "Maximum size of each log file in MB",
        default_value_t = 20
    )]
    pub logfile_max_size: u64,

    #[arg(
        long,
        global = true,
        value_name = "NUMBER",
        help = "Maximum number of log files to keep",
        default_value_t = 5
    )]
    pub logfile_max_number: usize,

    #[arg(
        long,
        global = true,
        value_name = "DIR",
        help = "Directory path where the log file will be stored"
    )]
    pub logfile_dir: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        help = "If present, compress old log files. This can help reduce the space needed \
                to store old logs."
    )]
    pub logfile_compression: bool,
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

    // Enable logging based on the CLI
    let cli = Cli::parse();

    let _guard = enable_logging(&cli);

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

fn enable_logging(cli: &Cli) -> WorkerGuard {
    let filter_level: Level = cli.debug_level.into();

    let logger_config = LoggerConfig {
        path: cli.logfile_dir.clone(),
        debug_level: filter_level,
        max_log_size: cli.logfile_max_size,
        max_log_number: cli.logfile_max_number,
        compression: cli.logfile_compression,
    };

    let env_filter = EnvFilter::builder()
        .with_default_directive(filter_level.into())
        .from_env_lossy();

    let dependency_log_filter =
        FilterFn::new(filter_dependency_log as fn(&tracing::Metadata<'_>) -> bool);

    let (file_appender, guard) = init_file_logging(logger_config.clone());

    let file_layer = fmt::layer().with_writer(file_appender);

    if let Err(e) = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer())
        .with(dependency_log_filter)
        .with(file_layer)
        .try_init()
    {
        eprintln!("Failed to initialize logging: {e}");
    }

    guard
}
