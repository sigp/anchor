use std::{
    path::{Path, PathBuf},
    process,
};

use checks::{check_file, update_file};
use clap::{Command, Parser};
use docgen::anchor_command;
use errors::DocGenError;
use pagination::{generate_cli_page_content, generate_subcommand_page_content};

mod checks;
mod errors;
mod pagination;

#[derive(Parser)]
#[clap(
    name = "anchor-docgen",
    about = "CLI reference documentation generator for Anchor"
)]
struct DocGen {
    #[clap(subcommand)]
    command: Option<DocGenCommand>,
}

#[derive(Parser)]
enum DocGenCommand {
    /// Generate raw CLI reference content to stdout
    Generate,

    /// Update .mdx files with current CLI documentation
    Update {
        /// Path to the docs directory containing cli-*.mdx files
        #[clap(long, default_value = DOCS_PATH)]
        docs_dir: PathBuf,
    },

    /// Check if .mdx files match current CLI definitions
    Check {
        /// Path to the docs directory containing cli-*.mdx files
        #[clap(long, default_value = DOCS_PATH)]
        docs_dir: PathBuf,
    },
}

/// Path to the directory where auto-generated .mdx files are created.
const DOCS_PATH: &str = "docs/docs/pages";

/// Subcommand-to-file mapping for CLI reference pages.
const SUBCOMMAND_PAGES: [(&str, &str); 3] = [
    ("node", "cli-node.mdx"),
    ("keygen", "cli-keygen.mdx"),
    ("keysplit", "cli-keysplit.mdx"),
];

/// Renders documentation and updates/checks existing documentation based on the provided
/// `DocGenCommand`.
fn render_docs(
    docgen_command: Option<DocGenCommand>,
    anchor_command: &Command,
) -> Result<(), DocGenError> {
    match docgen_command.unwrap_or(DocGenCommand::Generate) {
        DocGenCommand::Generate => {
            let cli_content = generate_cli_page_content(&anchor_command)?;
            print!("{cli_content}");
            for (name, _) in SUBCOMMAND_PAGES {
                let content = generate_subcommand_page_content(&anchor_command, name)?;
                println!("---\n## {name}\n");
                print!("{content}");
            }
        }
        DocGenCommand::Update { docs_dir } => {
            run_update(&anchor_command, &docs_dir)?;
        }
        DocGenCommand::Check { docs_dir } => {
            run_check(&anchor_command, &docs_dir)?;
        }
    };
    Ok(())
}

fn main() {
    let args = DocGen::parse();
    let cmd = anchor_command();
    let result = render_docs(args.command, &cmd);

    if let Err(e) = result {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

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
    if let Err(e) = check_file(&docs_dir.join("cli.mdx"), &cli_content) {
        eprintln!("  {e}");
        out_of_date.push("cli.mdx".to_string());
    }

    for (name, file) in SUBCOMMAND_PAGES {
        let content = generate_subcommand_page_content(cmd, name)?;
        if let Err(e) = check_file(&docs_dir.join(file), &content) {
            eprintln!("  {e}");
            out_of_date.push(file.to_string());
        }
    }

    if out_of_date.is_empty() {
        eprintln!("CLI reference documentation is up to date.");
        Ok(())
    } else {
        Err(DocGenError::OutOfDate(
            docs_dir.join(out_of_date.join(", ")),
        ))
    }
}
