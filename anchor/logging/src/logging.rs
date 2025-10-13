//! Collection of logging logic for initialising Anchor.
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::Parser;
use logroller::{Compression, LogRollerBuilder, Rotation, RotationSize};
use tracing::Level;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};

pub use crate::tracing_libp2p_discv5_layer::{
    Libp2pDiscv5TracingLayer, create_libp2p_discv5_tracing_layer,
};

#[derive(Parser, Debug, Clone)]
pub struct FileLoggingFlags {
    #[arg(
        long,
        global = true,
        default_value_t = Level::DEBUG,
        value_parser = Level::from_str,
        help = "Specifies the verbosity level used when emitting logs to the log file")]
    pub logfile_debug_level: Level,

    #[arg(
        long,
        global = true,
        value_name = "SIZE",
        help = "Maximum size of each log file in MB. Set to 0 to disable file logging.",
        default_value_t = 50
    )]
    pub logfile_max_size: u64,

    #[arg(
        long,
        global = true,
        value_name = "NUMBER",
        help = "Maximum number of log files to keep. Set to 0 to disable file logging.",
        default_value_t = 100
    )]
    pub logfile_max_number: u64,

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

    #[arg(long, global = true, help = "Enables colors in logfile.")]
    pub logfile_color: bool,

    #[arg(
        long,
        global = true,
        default_value_t = Level::DEBUG,
        value_parser = Level::from_str,
        help = "Specifies the verbosity level used for the discv5 dependency log file")]
    pub discv5_log_level: Level,

    #[arg(
        long,
        global = true,
        default_value_t = Level::DEBUG,
        value_parser = Level::from_str,
        help = "Specifies the verbosity level used for the libp2p dependency log file. \
                Certain score penalty information is logged regardless of this setting.")]
    pub libp2p_log_level: Level,
}

impl FileLoggingFlags {
    pub fn disabled_file_logging(&self) -> bool {
        self.logfile_max_number == 0 || self.logfile_max_size == 0
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

pub fn init_file_logging(logs_dir: &Path, config: FileLoggingFlags) -> Option<LoggingLayer> {
    let filename = PathBuf::from("anchor.log");

    if config.disabled_file_logging() {
        return None;
    }

    let mut appender = LogRollerBuilder::new(logs_dir, &filename)
        .rotation(Rotation::SizeBased(RotationSize::MB(
            config.logfile_max_size,
        )))
        .max_keep_files(config.logfile_max_number);

    if config.logfile_compression {
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
