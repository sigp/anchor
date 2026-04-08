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
use render::{generate_cli_page_content, generate_subcommand_page_content};

/// Subcommand-to-file mapping for CLI reference pages.
const SUBCOMMAND_PAGES: [(&str, &str); 3] = [
    ("node", "cli-node.mdx"),
    ("keygen", "cli-keygen.mdx"),
    ("keysplit", "cli-keysplit.mdx"),
];

/// Updates the .mdx files with the latest CLI documentation.
fn run_update(cmd: &Command, docs_dir: &Path) -> Result<(), DocGenError> {
    let cli_content = generate_cli_page_content(cmd)?;
    update_file(&docs_dir.join("cli.mdx"), &cli_content)?;

    for (name, file) in SUBCOMMAND_PAGES {
        let content = generate_subcommand_page_content(cmd, name)?;
        update_file(&docs_dir.join(file), &content)?;
    }

    eprintln!("CLI reference documentation updated successfully.");
    Ok(())
}

/// Checks if the .mdx files are up to date with the current CLI definitions.
fn run_check(cmd: &Command, docs_dir: &Path) -> Result<(), DocGenError> {
    let mut out_of_date = Vec::new();

    let cli_content = generate_cli_page_content(cmd)?;
    match check_file(&docs_dir.join("cli.mdx"), &cli_content) {
        Ok(_) => {}
        Err(DocGenError::OutOfDate(_)) => {
            out_of_date.push("cli.mdx".to_string());
        }
        Err(e) => return Err(e),
    }

    for (name, file) in SUBCOMMAND_PAGES {
        let content = generate_subcommand_page_content(cmd, name)?;
        match check_file(&docs_dir.join(file), &content) {
            Ok(_) => {}
            Err(DocGenError::OutOfDate(_)) => {
                out_of_date.push(file.to_string());
            }
            Err(e) => return Err(e),
        }
    }

    if out_of_date.is_empty() {
        eprintln!("CLI reference documentation is up to date.");
        Ok(())
    } else {
        Err(DocGenError::OutOfDate(out_of_date.join(", ")))
    }
}

/// Renders documentation and updates/checks existing documentation based on the provided
/// `DocGenCommand`.
pub fn render_docs(
    docgen_command: Option<DocGenCommand>,
    anchor_command: &Command,
) -> Result<(), DocGenError> {
    match docgen_command.unwrap_or(DocGenCommand::Generate) {
        DocGenCommand::Generate => {
            let cli_content = generate_cli_page_content(anchor_command)?;
            print!("{cli_content}");
            for (name, _) in SUBCOMMAND_PAGES {
                let content = generate_subcommand_page_content(anchor_command, name)?;
                println!("---\n## {name}\n");
                print!("{content}");
            }
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
