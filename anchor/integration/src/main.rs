use crate::basic_sim::BasicSim;
use crate::cli::cli_app;
use tracing::error;
use tracing_subscriber::{filter::filter_fn, fmt, prelude::*, EnvFilter};

mod basic_sim;
mod checks;
mod cli;
mod local_anchor_node;
mod local_network;
mod mock_websocket;
mod util;

fn main() -> Result<(), String> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var(
            "RUST_LOG",
            "integration=debug,execution=debug,network=debug,client=debug,beacon_node_fallback=debug,anchor=debug,network=debug,qbft=debug",
        );
    }

    let env_filter = EnvFilter::from_env("RUST_LOG");

    let dep_log_filter = filter_fn(|metadata| {
        if let Some(file) = metadata.file() {
            !file.contains("/.cargo/")
        } else {
            true
        }
    });

    if let Err(e) = tracing_subscriber::registry()
        .with(env_filter)
        .with(dep_log_filter)
        .with(fmt::layer())
        .try_init()
    {
        eprintln!("Failed to initialize logging: {e}");
    }

    let matches = cli_app().get_matches();
    match matches.subcommand() {
        Some(("basic-sim", matches)) => {
            BasicSim::run(matches)?;
        }
        _ => {
            error!("Invalid subcommand. Use --help to see available options");
            std::process::exit(1)
        }
    }

    Ok(())
}
