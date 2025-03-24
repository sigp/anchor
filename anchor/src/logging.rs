//! Collection of logging logic for initialising Anchor.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use strum::Display;
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize, Display, ValueEnum)]
pub enum DebugLevel {
    #[strum(serialize = "info")]
    Info,
    #[strum(serialize = "debug")]
    Debug,
    #[strum(serialize = "trace")]
    Trace,
    #[strum(serialize = "warn")]
    Warn,
    #[strum(serialize = "error")]
    Error,
}

impl From<DebugLevel> for Level {
    fn from(debug_level: DebugLevel) -> Self {
        match debug_level {
            DebugLevel::Info => Level::INFO,
            DebugLevel::Debug => Level::DEBUG,
            DebugLevel::Trace => Level::TRACE,
            DebugLevel::Warn => Level::WARN,
            DebugLevel::Error => Level::ERROR,
        }
    }
}

/// Sets up the global tracing logging
pub fn enable_logging(debug_level: DebugLevel) {
    let filter_level: Level = debug_level.into();
    let env_filter = EnvFilter::builder()
        .with_default_directive(filter_level.into())
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}
