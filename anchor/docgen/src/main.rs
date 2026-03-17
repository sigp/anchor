use std::{
    path::{Path, PathBuf},
    process,
};

use checks::{check_file, update_file};
use clap::{Command, Parser};
use docgen::{construct_anchor_cli_tree, generate_markdown};
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
    /// Generate raw markdown documentation to stdout
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

const DOCS_PATH: &str = "docs/docs/pages";

/// Subcommand-to-file mapping for CLI reference pages.
const SUBCOMMAND_PAGES: [(&str, &str); 3] = [
    ("node", "cli-node.mdx"),
    ("keygen", "cli-keygen.mdx"),
    ("keysplit", "cli-keysplit.mdx"),
];

fn main() {
    let args = DocGen::parse();
    let cmd = construct_anchor_cli_tree();

    let result = match args.command.unwrap_or(DocGenCommand::Generate) {
        DocGenCommand::Generate => {
            let docs = generate_markdown(&cmd);
            print!("{docs}");
            Ok(())
        }
        DocGenCommand::Update { docs_dir } => run_update(&cmd, &docs_dir),
        DocGenCommand::Check { docs_dir } => run_check(&cmd, &docs_dir),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

/// Update the file at `path` with `content`, replacing the section between markers.
fn run_update(cmd: &Command, docs_dir: &Path) -> Result<(), DocGenError> {
    // Sequence:

    // Generate top-level help documentation for CLI tree provided in cmd and update the top-level
    // CLI doc file.
    let cli_content = generate_cli_page_content(cmd)?;
    update_file(&docs_dir.join("cli.mdx"), &cli_content)?;

    //
    for (name, file) in SUBCOMMAND_PAGES {
        let content = generate_subcommand_page_content(cmd, name)?;
        update_file(&docs_dir.join(file), &content)?;
    }

    eprintln!("CLI reference documentation updated successfully.");
    Ok(())
}

/// Check if the file at `path` matches the expected `content` between markers.
fn run_check(cmd: &Command, docs_dir: &Path) -> Result<(), DocGenError> {
    let mut out_of_date = Vec::new();

    let cli_content = generate_cli_page_content(cmd)?;
    if let Err(e) = check_file(&docs_dir.join("cli.mdx"), &cli_content) {
        eprintln!("  {e}");
        out_of_date.push(e);
    }

    for (name, file) in SUBCOMMAND_PAGES {
        let content = generate_subcommand_page_content(cmd, name)?;
        if let Err(e) = check_file(&docs_dir.join(file), &content) {
            eprintln!("  {e}");
            out_of_date.push(e);
        }
    }

    if out_of_date.is_empty() {
        eprintln!("CLI reference documentation is up to date.");
        Ok(())
    } else {
        eprintln!("\nCLI reference is out of date.");
        Err(out_of_date.remove(0))
    }
}
