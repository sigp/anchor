//! Collection of logging logic for initialising Anchor.
use clap::ValueEnum;
use client::config;
use logroller::{Compression, LogRollerBuilder, Rotation, RotationSize};
use serde::{Deserialize, Serialize};
use std::clone;
use std::path;
use std::path::PathBuf;
use strum::Display;
use tracing::Level;
use tracing_appender::non_blocking::NonBlocking;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggerConfig {
    pub path: Option<PathBuf>,
    #[serde(skip_serializing, skip_deserializing, default = "default_debug_level")]
    pub debug_level: Level,
    pub max_log_size: u64,
    pub max_log_number: usize,
}
impl Default for LoggerConfig {
    fn default() -> Self {
        LoggerConfig {
            path: Some(PathBuf::from("../../logs")),
            debug_level: Level::TRACE,
            max_log_size: 20,
            max_log_number: 5,
        }
    }
}

fn default_debug_level() -> Level {
    Level::INFO
}
pub struct LoggingLayer {
    pub non_blocking_writer: NonBlocking,
    pub guard: WorkerGuard,
}

pub fn init_file_logging(config: LoggerConfig /* */) -> (NonBlocking, WorkerGuard) {

    // let data_dir = dirs::home_dir()
    // .unwrap_or_else(|| PathBuf::from("."))
    // .join(".anchor")
    // .join(
    //     ssv_network
    //         .eth2_network
    //         .config
    //         .config_name
    //         .as_deref()
    //         .unwrap_or("custom"),
    // );

    // if log_path.is_none() {
    //     log_path = Some(
    //         parse_path_or_default(matches, "datadir")?
    //             .join(DEFAULT_BEACON_NODE_DIR)
    //             .join("logs"),
    //     );
    // }
    let file_path = config.path.clone();
    let filename = PathBuf::from("anchor.log");
    // if config.compression {
    //     appender = appender.compression(Compression::Gzip);
    // }
    let file_appender = match file_path {
        None => {
            eprintln!("No logfile path provided, logging to file is disabled");
            return tracing_appender::non_blocking(std::io::sink());
        }
        Some(_) if config.max_log_number == 0 || config.max_log_size == 0 => {
            // User has explicitly disabled logging to file, so don't emit a message.
            return tracing_appender::non_blocking(std::io::sink());
        }
        Some(path) => {
            let appender = LogRollerBuilder::new(path, filename)
                .rotation(Rotation::SizeBased(RotationSize::MB(config.max_log_size)))
                .max_keep_files(config.max_log_number.try_into().unwrap_or_else(|e| {
                    eprintln!("Failed to convert max_log_number to u64: {}", e);
                    10
                }));

            match appender.build() {
                Ok(file_appender) => file_appender,
                Err(e) => {
                    eprintln!("Failed to create rolling file appender: {e}");
                    return tracing_appender::non_blocking(std::io::sink());
                }
            }
        }
    };
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    (non_blocking, _guard)
}

pub fn filter_dependency_log(meta: &tracing::Metadata<'_>) -> bool {
    if let Some(file) = meta.file() {
        let target = meta.target();
        if file.contains("/.cargo/") {
            return target.contains("discv5") || target.contains("libp2p");
        } else {
            return !file.contains("gossipsub") && !target.contains("hyper");
        }
    }
    true
}