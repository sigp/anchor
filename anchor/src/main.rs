use clap::Parser;
use client::{Client, Node, config};
use environment::Environment;
use global_config::{GlobalConfig, GlobalFlags};
use keygen::Keygen;
use keysplit::Keysplit;
use task_executor::ShutdownReason;
use tracing::{error, info};
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

    let _guards = match logging::enable_logging(file_logging_flags, &global_config) {
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

    config.network.domain_type = config.global_config.ssv_network.ssv_domain_type;

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

    // Hold onto the data dir until shutdown to keep it locked.
    let _data_dir_guard = config.global_config.data_dir.clone();

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
