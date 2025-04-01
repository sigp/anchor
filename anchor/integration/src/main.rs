use crate::{
    basic_sim::BasicSim,
    cli::{Cli, Commands, SimConfig},
    util::setup_logging,
};
use clap::Parser;

mod basic_sim;
mod checks;
mod cli;
mod local_anchor_node;
mod local_network;
mod mock_websocket;
mod util;

fn main() -> Result<(), String> {
    setup_logging();

    // Parse cli config and get simulation config
    let cli = Cli::parse();
    let config = SimConfig::from(&cli.command);

    match cli.command {
        Commands::BasicSim { .. } => {
            BasicSim::run(config)?;
        }
    }

    Ok(())
}
