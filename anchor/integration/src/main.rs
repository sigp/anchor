use crate::basic_sim::BasicSim;
mod basic_sim;
mod local_network;
mod checks;

// The database comes preloaded with a cluster of 4 operators with one validator

fn main() -> Result<(), String>{
    BasicSim::run()?;
    Ok(())
}
