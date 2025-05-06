//! Collection of logging logic for initialising Anchor.
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use logroller::{Compression, LogRollerBuilder, Rotation, RotationSize};
use serde::{Deserialize, Serialize};
use strum::Display;
use tracing::Level;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};

pub use crate::tracing_libp2p_discv5_layer::{
    Libp2pDiscv5TracingLayer, create_libp2p_discv5_tracing_layer,
};

const MAX_LOG_SIZE: u64 = 20;
const MAX_LOG_NUMBER: usize = 5;
const DEFAULT_DEBUG_LEVEL: Level = Level::INFO;

#[derive(Clone)]
pub struct LoggerConfig {
    pub path: Option<PathBuf>,
    pub debug_level: Level,
    pub max_log_size: u64,
    pub max_log_number: usize,
    pub compression: bool,
}
impl Default for LoggerConfig {
    fn default() -> Self {
        LoggerConfig {
            path: None,
            debug_level: DEFAULT_DEBUG_LEVEL,
            max_log_size: MAX_LOG_SIZE,
            max_log_number: MAX_LOG_NUMBER,
            compression: false,
        }
    }
}

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

#[derive(Parser, Debug, Clone, Deserialize, Serialize)]
pub struct LoggingFlags {
    #[arg(
        long,
        global = true,
        default_value_t = DebugLevel::Info,
        help = "Specifies the verbosity level used when emitting logs to the terminal")]
    pub debug_level: DebugLevel,

    #[arg(
        long,
        global = true,
        default_value_t = DebugLevel::Debug,
        help = "Specifies the verbosity level used when emitting logs to the log file")]
    pub logfile_debug_level: DebugLevel,

    #[arg(
        long,
        global = true,
        value_name = "SIZE",
        help = "Maximum size of each log file in MB",
        default_value_t = 20
    )]
    pub logfile_max_size: u64,

    #[arg(
        long,
        global = true,
        value_name = "NUMBER",
        help = "Maximum number of log files to keep",
        default_value_t = 5
    )]
    pub logfile_max_number: usize,

    #[arg(
        long,
        global = true,
        value_name = "DIR",
        help = "Directory path where the log file will be stored"
    )]
    pub logfile_dir: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        help = "If present, compress old log files. This can help reduce the space needed \
                to store old logs."
    )]
    pub logfile_compression: bool,
}

pub struct LoggingLayer {
    pub non_blocking_writer: NonBlocking,
    pub guard: WorkerGuard,
}
impl LoggingLayer {
    pub fn new(non_blocking_writer: NonBlocking, guard: WorkerGuard) -> Self {
        Self {
            non_blocking_writer,
            guard,
        }
    }
}

pub fn init_file_logging(default_logs_dir: PathBuf, config: LoggerConfig) -> Option<LoggingLayer> {
    let filename = PathBuf::from("anchor.log");

    let path = if config.max_log_number == 0 || config.max_log_size == 0 {
        // User has explicitly disabled logging to file
        return None;
    } else {
        config.path.unwrap_or(default_logs_dir)
    };

    let mut appender = LogRollerBuilder::new(path, filename)
        .rotation(Rotation::SizeBased(RotationSize::MB(config.max_log_size)))
        .max_keep_files(config.max_log_number.try_into().unwrap_or_else(|e| {
            eprintln!("Failed to convert max_log_number to u64: {}", e);
            10
        }));

    if config.compression {
        appender = appender.compression(Compression::Gzip);
    }

    match appender.build() {
        Ok(file_appender) => {
            let (writer, guard) = tracing_appender::non_blocking(file_appender);
            Some(LoggingLayer::new(writer, guard))
        }
        Err(e) => {
            eprintln!("Failed to create rolling file appender: {e}");
            None
        }
    }
}
