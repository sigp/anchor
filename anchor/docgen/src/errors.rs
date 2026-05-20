use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DocGenError {
    #[error("Failed to read file {path}: {source}")]
    ReadFile { path: PathBuf, source: io::Error },

    #[error("Failed to write file {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },

    #[error("CLI documentation is out of date in {0}")]
    OutOfDate(String),

    #[error("Subcommand {subcommand} not found in CLI tree {cli_tree}")]
    SubcommandNotFound {
        subcommand: String,
        cli_tree: String,
    },
}
