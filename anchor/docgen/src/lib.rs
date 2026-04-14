mod checks;
mod errors;
mod format;
pub mod interface;
mod render;

use std::path::Path;

use checks::{check_file, update_file};
use clap::{Command, CommandFactory};
use cli::Cli;
use errors::DocGenError;
use interface::DocGenCommand;
use render::{generate_cli_reference_snippet, generate_subcommand_reference_snippet};

const CLI_REFERENCE_FILE: &str = "cli-global-options.mdx";

/// Subcommand-to-file mapping for generated CLI reference snippets.
const SUBCOMMAND_REFERENCE_PAGES: [(&str, &str); 3] = [
    ("node", "cli-node-options.mdx"),
    ("keygen", "cli-keygen-options.mdx"),
    ("keysplit", "cli-keysplit-options.mdx"),
];

/// Updates the generated reference snippets with the latest CLI documentation.
fn run_update(cmd: &Command, docs_dir: &Path) -> Result<(), DocGenError> {
    let cli_content = generate_cli_reference_snippet(cmd)?;
    update_file(&docs_dir.join(CLI_REFERENCE_FILE), &cli_content)?;

    for (name, file) in SUBCOMMAND_REFERENCE_PAGES {
        let content = generate_subcommand_reference_snippet(cmd, name)?;
        update_file(&docs_dir.join(file), &content)?;
    }

    eprintln!("CLI reference snippets updated successfully.");
    Ok(())
}

/// Checks if the generated reference snippets are up to date with the current CLI definitions.
fn run_check(cmd: &Command, docs_dir: &Path) -> Result<(), DocGenError> {
    let mut out_of_date = Vec::new();

    let cli_content = generate_cli_reference_snippet(cmd)?;
    match check_file(&docs_dir.join(CLI_REFERENCE_FILE), &cli_content) {
        Ok(_) => {}
        Err(DocGenError::OutOfDate(_)) => {
            out_of_date.push(CLI_REFERENCE_FILE.to_string());
        }
        Err(e) => return Err(e),
    }

    for (name, file) in SUBCOMMAND_REFERENCE_PAGES {
        let content = generate_subcommand_reference_snippet(cmd, name)?;
        match check_file(&docs_dir.join(file), &content) {
            Ok(_) => {}
            Err(DocGenError::OutOfDate(_)) => {
                out_of_date.push(file.to_string());
            }
            Err(e) => return Err(e),
        }
    }

    if out_of_date.is_empty() {
        eprintln!("CLI reference snippets are up to date.");
        Ok(())
    } else {
        Err(DocGenError::OutOfDate(out_of_date.join(", ")))
    }
}

/// Renders generated CLI reference snippets from the `clap` struct definitions to stdout.
fn display_help_docs(anchor_command: &Command) -> Result<(), DocGenError> {
    let cli_content = generate_cli_reference_snippet(anchor_command)?;
    println!("---\n# {CLI_REFERENCE_FILE}\n");
    print!("{cli_content}");
    for (name, file) in SUBCOMMAND_REFERENCE_PAGES {
        let content = generate_subcommand_reference_snippet(anchor_command, name)?;
        println!("\n---\n# {file}\n");
        print!("{content}");
    }
    Ok(())
}

/// Renders documentation and updates/checks existing documentation based on the provided
/// `DocGenCommand`.
pub fn render_docs(
    docgen_command: Option<DocGenCommand>,
    anchor_command: &Command,
) -> Result<(), DocGenError> {
    match docgen_command.unwrap_or(DocGenCommand::Generate) {
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
pub fn anchor_command() -> clap::Command {
    Cli::command()
}
