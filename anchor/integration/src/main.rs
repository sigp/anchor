use crate::basic_sim::BasicSim;
use crate::cli::cli_app;
use crate::util::setup_logging;
use tracing::error;

mod basic_sim;
mod checks;
mod cli;
mod local_anchor_node;
mod local_network;
mod mock_websocket;
mod util;

fn main() -> Result<(), String> {
    setup_logging();

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
