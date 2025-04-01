use clap::{Parser, Subcommand, crate_version};

#[derive(Parser)]
#[command(name = "simulator")]
#[command(version = crate_version!())]
#[command(author = "Sigma Prime <contact@sigmaprime.io>")]
#[command(about = "Options for interacting with simulator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Runs a ssv validator simulation")]
    BasicSim {
        #[arg(
            short = 'n',
            long = "nodes",
            default_value = "4",
            help = "Number of beacon nodes"
        )]
        nodes: String,

        #[arg(
            short = 'p',
            long = "proposer-nodes",
            default_value = "4",
            help = "Number of proposer-only beacon nodes"
        )]
        proposer_nodes: String,

        #[arg(
            short = 'v',
            long = "validators-per-node",
            default_value = "256",
            help = "Number of validators"
        )]
        validators_per_node: String,

        #[arg(
            short = 's',
            long = "speed-up-factor",
            default_value = "3",
            help = "Speed up factor. Please use a divisor of 12."
        )]
        speed_up_factor: String,

        #[arg(
            short = 'd',
            long = "debug-level",
            default_value = "debug",
            help = "Set the severity level of the logs."
        )]
        debug_level: String,

        #[arg(
            short = 'c',
            long = "continue_after_checks",
            help = "Continue after checks (default false)"
        )]
        continue_after_checks: bool,
    },
}

pub struct SimConfig {
    pub node_count: usize,
    pub proposer_nodes: usize,
    pub validators_per_node: usize,
    pub speed_up_factor: u64,
    pub log_level: String,
    pub committee_size: usize,
    pub continue_after_checks: bool,
}

impl From<&Commands> for SimConfig {
    fn from(command: &Commands) -> Self {
        match command {
            Commands::BasicSim {
                nodes,
                proposer_nodes,
                validators_per_node,
                speed_up_factor,
                debug_level,
                continue_after_checks,
            } => {
                let node_count = nodes.parse::<usize>().expect("Failed to parse nodes");

                let proposer_node_count = proposer_nodes
                    .parse::<usize>()
                    .expect("Failed to parse proposer-nodes");

                let validators = validators_per_node
                    .parse::<usize>()
                    .expect("Failed to parse validators-per-node");

                let speed = speed_up_factor
                    .parse::<u64>()
                    .expect("Failed to parse speed-up-factor");

                SimConfig {
                    node_count,
                    proposer_nodes: proposer_node_count,
                    validators_per_node: validators,
                    speed_up_factor: speed,
                    log_level: debug_level.clone(),
                    committee_size: 4,
                    continue_after_checks: *continue_after_checks,
                }
            }
        }
    }
}
