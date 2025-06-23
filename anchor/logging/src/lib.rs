//! Collection of logging logic for initialising Anchor.

use std::path::{Path, PathBuf};

use clap::Parser;
use count_layer::CountLayer;
use global_config::{DebugLevel, GlobalConfig};
use logroller::{Compression, LogRollerBuilder, Rotation, RotationSize};
use tracing::Level;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    tracing_libp2p_discv5_layer::create_libp2p_discv5_tracing_layer, utils::build_workspace_filter,
};

mod count_layer;
mod tracing_libp2p_discv5_layer;
mod utils;

#[derive(Parser, Debug, Clone)]
pub struct FileLoggingFlags {
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

pub fn init_file_logging(
    logs_dir: &Path,
    config: &FileLoggingFlags,
) -> Result<Option<LoggingLayer>, String> {
    if config.disabled_file_logging() {
        return Ok(None);
    }

    let filename = PathBuf::from("anchor.log");
    let mut appender = LogRollerBuilder::new(logs_dir, &filename)
        .rotation(Rotation::SizeBased(RotationSize::MB(
            config.logfile_max_size,
        )))
        .max_keep_files(config.logfile_max_number);

    if config.logfile_compression {
        appender = appender.compression(Compression::Gzip);
    }

    let file_appender = appender
        .build()
        .map_err(|e| format!("Failed to create rolling file appender: {e}"))?;

    let (writer, guard) = tracing_appender::non_blocking(file_appender);
    Ok(Some(LoggingLayer::new(writer, guard)))
}

pub fn enable_logging(
    file_logging_flags: Option<&FileLoggingFlags>,
    global_config: &GlobalConfig,
) -> Result<Vec<WorkerGuard>, String> {
    let mut logging_layers = Vec::new();
    let mut guards = Vec::new();

    let workspace_filter = match build_workspace_filter() {
        Ok(filter) => filter,
        Err(e) => {
            return Err(format!("Unable to build workspace filter: {e}"));
        }
    };

    logging_layers.push(
        fmt::layer()
            .with_filter(
                EnvFilter::builder()
                    .with_default_directive(global_config.debug_level.into())
                    .from_env_lossy(),
            )
            .with_filter(workspace_filter.clone())
            .boxed(),
    );

    if let Some(file_logging_flags) = file_logging_flags {
        let logs_dir = file_logging_flags
            .logfile_dir
            .clone()
            .unwrap_or_else(|| global_config.data_dir.join("logs"));

        let filter_level: Level = file_logging_flags.logfile_debug_level.into();

        let libp2p_discv5_layer =
            create_libp2p_discv5_tracing_layer(&logs_dir, file_logging_flags)?;
        let file_logging_layer = init_file_logging(&logs_dir, file_logging_flags)?;

        if let Some((libp2p_discv5_layer, layer_guards)) = libp2p_discv5_layer {
            guards.extend(layer_guards);
            logging_layers.push(
                libp2p_discv5_layer
                    .with_filter(
                        EnvFilter::builder()
                            .with_default_directive(Level::DEBUG.into())
                            .from_env_lossy(),
                    )
                    .boxed(),
            );
        }

        if let Some(file_logging_layer) = file_logging_layer {
            guards.push(file_logging_layer.guard);
            logging_layers.push(
                fmt::layer()
                    .with_writer(file_logging_layer.non_blocking_writer)
                    .with_ansi(file_logging_flags.logfile_color)
                    .with_filter(
                        EnvFilter::builder()
                            .with_default_directive(filter_level.into())
                            .from_env_lossy(),
                    )
                    .with_filter(workspace_filter.clone())
                    .boxed(),
            );
        }
    }

    // Add the CountLayer
    logging_layers.push(CountLayer.boxed());

    tracing_subscriber::registry()
        .with(logging_layers)
        .try_init()
        .map_err(|e| format!("Failed to initialize logging: {e}"))?;

    Ok(guards)
}
