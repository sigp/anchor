mod checks;
mod errors;
pub mod interface;
mod render;

use std::path::Path;

use checks::{append_to_out_of_date_files, check_out_of_date_files, update_file};
use clap::{Command, CommandFactory};
use cli::Cli;
use errors::DocGenError;
use interface::DocGenCommand;
use render::{render_help_flat, render_subcommand_help_snippet, to_title_case};

const CLI_REFERENCE_FILE: &str = "cli-global-options.mdx";

/// Subcommand-to-file mapping for generated CLI reference snippets.
const SUBCOMMAND_REFERENCE_PAGES: [(&str, &str); 3] = [
    ("node", "cli-node-options.mdx"),
    ("keygen", "cli-keygen-options.mdx"),
    ("keysplit", "cli-keysplit-options.mdx"),
];

/// Updates the generated reference snippets with the latest CLI documentation.
fn run_update(cmd: &mut Command, docs_dir: &Path) -> Result<(), DocGenError> {
    // Recommended by clap: https://docs.rs/clap/latest/clap/struct.Command.html#method.build.
    // This forces the tree to be fully constructed before we start updating files, any further
    // changes invoked in clap's internals to the command tree are no-op.
    cmd.build();

    let cli_content = render_help_flat(cmd);
    update_file(&docs_dir.join(CLI_REFERENCE_FILE), &cli_content)?;

    let cli_name = cmd.get_name().to_string();
    for (name, file) in SUBCOMMAND_REFERENCE_PAGES {
        let content = render_subcommand_help_snippet(cmd, name, &cli_name)?;
        update_file(&docs_dir.join(file), &content)?;
    }

    println!("CLI reference snippets updated successfully.");
    Ok(())
}

/// Checks if the generated reference snippets are up to date with the current CLI definitions.
fn run_check(cmd: &mut Command, docs_dir: &Path) -> Result<(), DocGenError> {
    // Recommended by clap: https://docs.rs/clap/latest/clap/struct.Command.html#method.build.
    // This forces the tree to be fully constructed before we start updating files, any further
    // changes invoked in clap's internals to the command tree are no-op.
    cmd.build();

    let mut out_of_date = Vec::new();

    let cli_content = render_help_flat(cmd);
    append_to_out_of_date_files(
        &docs_dir.join(CLI_REFERENCE_FILE),
        CLI_REFERENCE_FILE,
        &cli_content,
        &mut out_of_date,
    )?;

    let cli_name = cmd.get_name().to_string();
    for (name, file) in SUBCOMMAND_REFERENCE_PAGES {
        let content = render_subcommand_help_snippet(cmd, name, &cli_name)?;
        append_to_out_of_date_files(&docs_dir.join(file), file, &content, &mut out_of_date)?;
    }

    check_out_of_date_files(&out_of_date)
}

/// Renders generated CLI reference snippets from the `clap` struct definitions to stdout.
fn display_help_docs(anchor_command: &mut Command) -> Result<(), DocGenError> {
    let cli_content = render_help_flat(anchor_command);
    println!("---\n# Global Options\n");
    print!("{cli_content}");
    let cli_name = anchor_command.get_name().to_string();
    for (name, _) in SUBCOMMAND_REFERENCE_PAGES {
        let content = render_subcommand_help_snippet(anchor_command, name, &cli_name)?;
        let title = format!("{} Command", to_title_case(name));
        println!("\n---\n# {title}\n");
        print!("{content}");
    }
    Ok(())
}

/// Renders documentation and updates/checks existing documentation based on the provided
/// `DocGenCommand`.
pub fn render_docs(
    docgen_command: DocGenCommand,
    anchor_command: &mut Command,
) -> Result<(), DocGenError> {
    match docgen_command {
        DocGenCommand::Generate => {
            display_help_docs(anchor_command)?;
        }
        DocGenCommand::Update { docs_dir } => {
            run_update(anchor_command, &docs_dir)?;
        }
        DocGenCommand::Check { docs_dir } => {
            run_check(anchor_command, &docs_dir)?;
        }
    };
    Ok(())
}

/// Returns the fully-built `clap::Command` tree for the Anchor CLI.
///
/// This is the single source of truth — no parallel tree reconstruction needed.
pub fn anchor_command() -> Command {
    Cli::command()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_update_then_check_agrees() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = anchor_command();

        run_update(&mut cmd, dir.path()).unwrap();

        let mut cmd = anchor_command();
        run_check(&mut cmd, dir.path()).unwrap();
    }

    #[test]
    fn test_run_check_detects_stale_files() {
        let dir = tempfile::tempdir().unwrap();

        // Write stale content to the global options file.
        let stale_path = dir.path().join(CLI_REFERENCE_FILE);
        std::fs::write(&stale_path, "stale content").unwrap();

        let mut cmd = anchor_command();
        let result = run_check(&mut cmd, dir.path());

        assert!(
            matches!(result, Err(DocGenError::OutOfDate(ref msg)) if msg.contains(CLI_REFERENCE_FILE)),
            "Expected OutOfDate error mentioning {CLI_REFERENCE_FILE}, got: {result:?}"
        );
    }
}
