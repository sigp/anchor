use std::{
    io::Write,
    path::{Path, PathBuf},
};

use chrono::Local;
use logroller::{LogRollerBuilder, Rotation, RotationSize};
use tracing::Subscriber;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::{Layer, layer::Context};

use crate::FileLoggingFlags;

pub struct Libp2pDiscv5TracingLayer {
    pub libp2p_non_blocking_writer: NonBlocking,
    pub discv5_non_blocking_writer: NonBlocking,
}

impl<S> Layer<S> for Libp2pDiscv5TracingLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<S>) {
        let meta = event.metadata();
        let log_level = meta.level();
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let target = match meta.target().split_once("::") {
            Some((crate_name, _)) => crate_name,
            None => "unknown",
        };

        let mut writer = match target {
            "libp2p_gossipsub" => self.libp2p_non_blocking_writer.clone(),
            "discv5" => self.discv5_non_blocking_writer.clone(),
            _ => return,
        };

        let mut visitor = LogMessageExtractor {
            message: String::default(),
        };

        event.record(&mut visitor);
        let message = format!("{} {} {}\n", timestamp, log_level, visitor.message);

        if let Err(e) = writer.write_all(message.as_bytes()) {
            eprintln!("Failed to write log: {e}");
        }
    }
}

struct LogMessageExtractor {
    message: String,
}

impl tracing_core::field::Visit for LogMessageExtractor {
    fn record_debug(&mut self, _: &tracing_core::Field, value: &dyn std::fmt::Debug) {
        self.message = format!("{} {:?}", self.message, value);
    }
}

pub fn create_libp2p_discv5_tracing_layer(
    logs_dir: &Path,
    logging_config: &FileLoggingFlags,
) -> Result<Option<(Libp2pDiscv5TracingLayer, [WorkerGuard; 2])>, String> {
    if logging_config.disabled_file_logging() {
        return Ok(None);
    }

    let libp2p_writer = LogRollerBuilder::new(logs_dir, &PathBuf::from("libp2p.log"))
        .rotation(Rotation::SizeBased(RotationSize::MB(
            logging_config.logfile_max_size,
        )))
        .max_keep_files(logging_config.logfile_max_number);

    let discv5_writer = LogRollerBuilder::new(logs_dir, &PathBuf::from("discv5.log"))
        .rotation(Rotation::SizeBased(RotationSize::MB(
            logging_config.logfile_max_size,
        )))
        .max_keep_files(logging_config.logfile_max_number);

    let libp2p_writer = libp2p_writer
        .build()
        .map_err(|e| format!("Failed to initialize libp2p rolling file appender: {e}"))?;

    let discv5_writer = discv5_writer
        .build()
        .map_err(|e| format!("Failed to initialize discv5 rolling file appender: {e}"))?;

    let (libp2p_non_blocking_writer, libp2p_guard) = NonBlocking::new(libp2p_writer);
    let (discv5_non_blocking_writer, discv5_guard) = NonBlocking::new(discv5_writer);

    Ok(Some((
        Libp2pDiscv5TracingLayer {
            libp2p_non_blocking_writer,
            discv5_non_blocking_writer,
        },
        [libp2p_guard, discv5_guard],
    )))
}
