//! Collection of logging logic for initialising Anchor.
use std::path::PathBuf;

use logroller::{Compression, LogRollerBuilder, Rotation, RotationSize};
use tracing::Level;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};

pub use crate::tracing_libp2p_discv5_layer::{
    create_libp2p_discv5_tracing_layer, Libp2pDiscv5TracingLayer,
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

pub fn filter_dependency_log(meta: &tracing::Metadata<'_>) -> bool {
    if let Some(file) = meta.file() {
        let target = meta.target();
        if file.contains("/.cargo/") {
            return target.contains("discv5") || target.contains("libp2p");
        }
    }
    true
}
