use clap::CommandFactory;
use cli::Cli;

/// Returns the fully-built `clap::Command` tree for the Anchor CLI.
///
/// This is the single source of truth — no parallel tree reconstruction needed.
pub fn anchor_command() -> clap::Command {
    Cli::command()
}
