use crate::basic_sim::BasicSim;
use crate::cli::cli_app;
use env_logger::{Builder, Env};
use tracing::error;

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
            "integration=debug,execution=debug,client=debug,beacon_node_fallback=debug,anchor=debug,network=debug",
        );
    }
    Builder::from_env(Env::default()).init();

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
