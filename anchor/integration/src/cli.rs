use clap::{crate_version, Arg, ArgAction, Command};

pub fn cli_app() -> Command {
    Command::new("simulator")
        .version(crate_version!())
        .author("Sigma Prime <contact@sigmaprime.io>")
        .about("Options for interacting with simulator")
        .subcommand(
            Command::new("basic-sim")
                .about("Runs a ssv validator simulation")
                .arg(
                    Arg::new("nodes")
                        .short('n')
                        .long("nodes")
                        .action(ArgAction::Set)
                        .default_value("4")
                        .help("Number of beacon nodes"),
                )
                .arg(
                    Arg::new("proposer-nodes")
                        .short('p')
                        .long("proposer-nodes")
                        .action(ArgAction::Set)
                        .default_value("4")
                        .help("Number of proposer-only beacon nodes"),
                )
                .arg(
                    Arg::new("validators-per-node")
                        .short('v')
                        .long("validators-per-node")
                        .action(ArgAction::Set)
                        .default_value("32")
                        .help("Number of validators"),
                )
                .arg(
                    Arg::new("speed-up-factor")
                        .short('s')
                        .long("speed-up-factor")
                        .action(ArgAction::Set)
                        .default_value("3")
                        .help("Speed up factor. Please use a divisor of 12."),
                )
                .arg(
                    Arg::new("debug-level")
                        .short('d')
                        .long("debug-level")
                        .action(ArgAction::Set)
                        .default_value("debug")
                        .help("Set the severity level of the logs."),
                )
                .arg(
                    Arg::new("continue-after-checks")
                        .short('c')
                        .long("continue_after_checks")
                        .action(ArgAction::SetTrue)
                        .help("Continue after checks (default false)"),
                ),
        )
}
