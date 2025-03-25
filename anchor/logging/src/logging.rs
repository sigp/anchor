//! Collection of logging logic for initialising Anchor.
use clap::ValueEnum;
use logroller::{Compression, LogRollerBuilder, Rotation, RotationSize};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use strum::Display;
use tracing::Level;
use tracing_appender::non_blocking::NonBlocking;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggerConfig {
    pub path: Option<PathBuf>,
    #[serde(skip_serializing, skip_deserializing, default = "default_debug_level")]
    pub debug_level: LevelFilter,
    #[serde(
        skip_serializing,
        skip_deserializing,
        default = "default_logfile_debug_level"
    )]
    pub logfile_debug_level: LevelFilter,
    pub log_format: Option<String>,
    pub logfile_format: Option<String>,
    pub log_color: bool,
    pub logfile_color: bool,
    pub disable_log_timestamp: bool,
    pub max_log_size: u64,
    pub max_log_number: usize,
    pub compression: bool,
    pub is_restricted: bool,
    pub sse_logging: bool,
    pub extra_info: bool,
}
impl Default for LoggerConfig {
    fn default() -> Self {
        LoggerConfig {
            path: Some(PathBuf::from("../logs")),
            debug_level: LevelFilter::TRACE,
            logfile_debug_level: LevelFilter::TRACE,
            log_format: None,
            log_color: true,
            logfile_format: None,
            logfile_color: false,
            disable_log_timestamp: false,
            max_log_size: 200,
            max_log_number: 5,
            compression: false,
            is_restricted: true,
            sse_logging: false,
            extra_info: false,
        }
    }
}

fn default_debug_level() -> LevelFilter {
    LevelFilter::INFO
}

fn default_logfile_debug_level() -> LevelFilter {
    LevelFilter::DEBUG
}

pub struct LoggingLayer {
    pub non_blocking_writer: NonBlocking,
    pub guard: WorkerGuard,
}

pub fn init_file_logging(config: LoggerConfig) -> (NonBlocking, WorkerGuard) {
    let file_path = config.path.unwrap_or_else(|| PathBuf::from("."));
    let filename = PathBuf::from("anchor.log");

    let mut appender = LogRollerBuilder::new(file_path, filename)
        .rotation(Rotation::SizeBased(RotationSize::MB(config.max_log_size)))
        .max_keep_files(config.max_log_number.try_into().unwrap_or_else(|e| {
            eprintln!("Failed to convert max_log_number to u64: {}", e);
            10
        }));

    if config.compression {
        appender = appender.compression(Compression::Gzip);
    }

    let file_appender = match appender.build() {
        Ok(file_appender) => file_appender,
        Err(e) => {
            eprintln!("Failed to create rolling file appender: {e}");
            return tracing_appender::non_blocking(std::io::sink());
        }
    };
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    (non_blocking, _guard)
}
