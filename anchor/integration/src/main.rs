use crate::basic_sim::BasicSim;
mod basic_sim;
mod checks;
mod local_network;
use clap::{ArgMatches, Command};

// The database comes preloaded with a cluster of 4 operators with one validator
//
fn get_matches() -> Command {
    todo!()
}

fn main() -> Result<(), String> {
    let res = ArgMatches::default();

    BasicSim::run(&res)?;
    Ok(())
}
