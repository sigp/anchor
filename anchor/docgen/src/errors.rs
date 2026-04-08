use std::{fmt, io, path::PathBuf};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DocGenError {
    #[error("Failed to read file {path}: {source}")]
    ReadFile { path: PathBuf, source: io::Error },

    #[error("Failed to write file {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },

    #[error("Missing {marker} marker in {path}")]
    MissingMarker { path: PathBuf, marker: String },

    #[error("CLI documentation is out of date in {0}")]
    OutOfDate(String),

    #[error("Unable to render documentation for option group {group}: {source}")]
    RenderOptionGroup { group: String, source: fmt::Error },

    #[error("Subcommand {subcommand} not found in CLI tree {cli_tree}")]
    SubcommandNotFound {
        subcommand: String,
        cli_tree: String,
    },

    #[error("Sentinel markers are in the incorrect order for {path}")]
    InvalidMarkers { path: PathBuf },
}
