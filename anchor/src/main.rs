use clap::Parser;
use tracing::{error, info};

mod environment;
use client::cli::DebugLevel;
use client::{config, Client, Node};
use environment::Environment;
use keygen::Keygen;
use keysplit::Keysplit;
use logging::logging::{init_file_logging, LoggerConfig};
use std::path::PathBuf;
use task_executor::ShutdownReason;
use tracing::Level;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;
use types::EthSpecId;

#[derive(Parser, Clone, Debug)]
struct Cli {
    #[clap(subcommand)]
    pub subcommand: AnchorSubcommands,
    #[arg(long, default_value_t = DebugLevel::Info, help = "Specifies the verbosity level used when emitting logs to the terminal")]
    pub debug_level: DebugLevel,

    #[arg(
        long,
        global = true,
        help = "Directory path where the log files will be stored"
    )]
    pub log_path: Option<PathBuf>,

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
    let filter_level: Level = cli.debug_level.into();
    // TODO: massive tidying up to do here
    let logger_config = LoggerConfig {
        path: cli.log_path,
        debug_level: filter_level,
        max_log_size: cli.logfile_max_size,
        max_log_number: cli.logfile_max_number,
        // compression: Compression::None,
    };

    let env_filter = EnvFilter::builder()
        .with_default_directive(filter_level.into())
        .from_env_lossy();

    let (file_appender, _guard) = init_file_logging(logger_config.clone());
    let libp2p_discv5_layer = logging::create_libp2p_discv5_tracing_layer(
        logger_config.path.clone(),
        logger_config.max_log_size,
        // logger_config.compression,
        logger_config.max_log_number,
    );
    let file_layer = fmt::layer().with_writer(file_appender);
    if let Err(e) = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer())
        .with(libp2p_discv5_layer)
        .with(file_layer)
        .try_init()
    {
        eprintln!("Failed to initialize logging: {e}");
    }

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
