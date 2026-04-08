//! CLI interface for the docgen crate.

use std::path::PathBuf;

use clap::Parser;

/// Path to the directory where auto-generated .mdx files are created.
const DOCS_PATH: &str = "docs/docs/pages";

#[derive(Parser)]
#[clap(
    name = "anchor-docgen",
    about = "CLI reference documentation generator for Anchor"
)]
pub struct DocGen {
    #[clap(subcommand)]
    pub command: Option<DocGenCommand>,
}

#[derive(Parser)]
pub enum DocGenCommand {
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
