//! CLI interface for the docgen crate.

use std::path::PathBuf;

use clap::Parser;

/// Path to the directory where auto-generated .mdx files are created.
const DOCS_PATH: &str = "docs/docs/generated";

#[derive(Parser)]
#[clap(
    name = "anchor-docgen",
    about = "CLI reference documentation generator for Anchor",
    next_line_help = true,
    term_width = 80,
    display_order = 0
)]
pub struct DocGen {
    #[clap(subcommand)]
    pub command: DocGenCommand,
}

#[derive(Parser)]
pub enum DocGenCommand {
    /// Generate CLI reference snippets to stdout
    Generate,

    /// Update generated reference snippets
    Update {
        /// Path to the docs directory containing generated CLI snippet .mdx files
        #[clap(long, default_value = DOCS_PATH)]
        docs_dir: PathBuf,
    },

    /// Check if generated reference snippets match current CLI definitions
    Check {
        /// Path to the docs directory containing generated CLI snippet .mdx files
        #[clap(long, default_value = DOCS_PATH)]
        docs_dir: PathBuf,
    },
}
