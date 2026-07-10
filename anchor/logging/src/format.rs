//! Anchor logging format
//!
//! This file contains the logging formatting options used to display anchor logs.

use std::fmt;

use nu_ansi_term::{Color, Style};
use tracing_core::{Event, Level, Subscriber};
use tracing_subscriber::{
    field::MakeVisitor,
    fmt::{
        FmtContext, FormattedFields,
        format::{self, FormatEvent, FormatFields, Writer},
        time::{FormatTime, SystemTime},
    },
    registry::LookupSpan,
};

#[derive(Clone)]
pub struct AnchorFormatter {
    timer: SystemTime,
    ansi: bool,
    display_target: bool,
}

impl AnchorFormatter {
    pub fn new() -> Self {
        Self {
            timer: SystemTime,
            ansi: true,
            display_target: false,
        }
    }

    pub fn with_ansi(mut self, ansi: bool) -> Self {
        self.ansi = ansi;
        self
    }

    pub fn with_target(mut self) -> Self {
        self.display_target = true;
        self
    }
}

impl Default for AnchorFormatter {
    fn default() -> Self {
        Self::new()
    }
}

struct FieldCapture {
    message: Option<String>,
    other_fields: Vec<(String, String)>,
}

impl FieldCapture {
    fn new() -> Self {
        Self {
            message: None,
            other_fields: Vec::new(),
        }
    }
}

impl tracing_core::field::Visit for FieldCapture {
    fn record_debug(&mut self, field: &tracing_core::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{:?}", value));
        } else {
            self.other_fields
                .push((field.name().to_string(), format!("{:?}", value)));
        }
    }
}

fn format_level(level: &Level, writer: &mut Writer<'_>, use_ansi: bool) -> fmt::Result {
    if use_ansi && writer.has_ansi_escapes() {
        match *level {
            Level::TRACE => write!(writer, "{}", Color::Purple.paint("TRACE")),
            Level::DEBUG => write!(writer, "{}", Color::Blue.paint("DEBUG")),
            Level::INFO => write!(writer, "{}", Color::Green.paint(" INFO")),
            Level::WARN => write!(writer, "{}", Color::Yellow.paint(" WARN")),
            Level::ERROR => write!(writer, "{}", Color::Red.paint("ERROR")),
        }?;
    } else {
        write!(writer, "{:5}", level)?;
    }
    Ok(())
}

impl<S, N> FormatEvent<S, N> for AnchorFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();

        if self.ansi && writer.has_ansi_escapes() {
            let style = Style::new().dimmed();
            write!(writer, "{}", style.prefix())?;
            let _ = self.timer.format_time(&mut writer);
            write!(writer, "{} ", style.suffix())?;
        } else {
            let _ = self.timer.format_time(&mut writer);
            writer.write_char(' ')?;
        }

        format_level(meta.level(), &mut writer, self.ansi)?;
        writer.write_char(' ')?;

        let mut field_capture = FieldCapture::new();
        event.record(&mut field_capture);

        let message_str = if let Some(msg) = field_capture.message {
            msg.trim_matches('"').to_string()
        } else {
            String::new()
        };

        write!(writer, "{}", message_str)?;

        let has_fields = !field_capture.other_fields.is_empty() || self.display_target;

        if has_fields {
            const COLUMN_WIDTH: usize = 50;
            let message_len = message_str.chars().count();

            if message_len < COLUMN_WIDTH {
                let padding = COLUMN_WIDTH - message_len;
                for _ in 0..padding {
                    writer.write_char(' ')?;
                }
            } else {
                writer.write_char(' ')?;
            }

            let mut first = true;

            if self.display_target {
                if self.ansi && writer.has_ansi_escapes() {
                    let dimmed = Style::new().dimmed();
                    let italic = Style::new().italic();
                    write!(
                        writer,
                        "{}{}\"{}\"",
                        italic.paint("target"),
                        dimmed.paint("="),
                        meta.target()
                    )?;
                } else {
                    write!(writer, "target=\"{}\"", meta.target())?;
                }
                first = false;
            }

            if self.ansi && writer.has_ansi_escapes() {
                let dimmed = Style::new().dimmed();
                let italic = Style::new().italic();

                for (name, value) in field_capture.other_fields.iter() {
                    if !first {
                        writer.write_char(' ')?;
                    }
                    first = false;
                    write!(
                        writer,
                        "{}{}{}",
                        italic.paint(name),
                        dimmed.paint("="),
                        value
                    )?;
                }
            } else {
                for (name, value) in field_capture.other_fields.iter() {
                    if !first {
                        writer.write_char(' ')?;
                    }
                    first = false;
                    write!(writer, "{}={}", name, value)?;
                }
            }
        }

        for span in ctx
            .event_scope()
            .into_iter()
            .flat_map(|scope| scope.from_root())
        {
            let exts = span.extensions();
            if let Some(fields) = exts.get::<FormattedFields<N>>()
                && !fields.is_empty()
            {
                if self.ansi && writer.has_ansi_escapes() {
                    let dimmed = Style::new().dimmed();
                    write!(writer, " {}", dimmed.paint(&fields.fields))?;
                } else {
                    write!(writer, " {}", fields.fields)?;
                }
            }
        }

        writeln!(writer)
    }
}

/// Field formatter for the file logging layer.
///
/// This type exists solely to give the file layer a separate
/// `FormattedFields<FileFields>` cache slot in the span extensions type-map,
/// distinct from the console layer's default `FormattedFields<DefaultFields>`.
/// Without this, the console layer (ANSI-enabled) caches span fields first,
/// and the file layer reuses those ANSI-contaminated fields.
pub struct FileFields;

impl<'writer> MakeVisitor<Writer<'writer>> for FileFields {
    type Visitor = format::DefaultVisitor<'writer>;

    fn make_visitor(&self, target: Writer<'writer>) -> Self::Visitor {
        format::DefaultFields::new().make_visitor(target)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::{fmt, layer::SubscriberExt};

    use super::*;

    /// ANSI escape sequence prefix. Any output destined for a plain-text file
    /// must never contain this byte sequence.
    const ANSI_ESCAPE: &[u8] = b"\x1b[";

    /// Returns true if the byte slice contains an ANSI escape sequence.
    fn contains_ansi(bytes: &[u8]) -> bool {
        bytes
            .windows(ANSI_ESCAPE.len())
            .any(|window| window == ANSI_ESCAPE)
    }

    /// Helper: build a shared in-memory buffer suitable as a `MakeWriter`.
    fn shared_buffer() -> Arc<Mutex<Vec<u8>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    /// Verifies that the `FileFields` fix prevents ANSI escape sequences from
    /// leaking into file layer output when two layers share a registry.
    ///
    /// Setup mirrors production: a console layer (ANSI-enabled, `DefaultFields`)
    /// and a file layer (ANSI-disabled, `FileFields`). Both are attached to the
    /// same `Registry`. Because `FileFields` has a distinct `TypeId` from
    /// `DefaultFields`, each layer gets its own `FormattedFields<_>` cache slot
    /// in the span extensions. The file layer therefore never reads the
    /// console layer's ANSI-contaminated cached fields.
    ///
    /// This test PASSES with the fix (`FileFields`) in place.
    #[test]
    fn test_file_fields_no_ansi_leak_in_span_fields() {
        // Arrange
        let console_buf = shared_buffer();
        let file_buf = shared_buffer();

        let console_writer = console_buf.clone();
        let file_writer = file_buf.clone();

        // Console layer: ANSI enabled, uses default DefaultFields (implicit).
        let console_layer =
            fmt::layer()
                .with_ansi(true)
                .with_writer(move || -> Box<dyn std::io::Write> {
                    Box::new(MockWriter(console_writer.clone()))
                });

        // File layer: ANSI disabled, uses FileFields for a distinct cache slot.
        let file_layer = fmt::layer()
            .with_ansi(false)
            .fmt_fields(FileFields)
            .with_writer(move || -> Box<dyn std::io::Write> {
                Box::new(MockWriter(file_writer.clone()))
            });

        let subscriber = tracing_subscriber::registry()
            .with(console_layer)
            .with(file_layer);

        // Act
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("test_span", some_field = "hello_world");
            let _guard = span.enter();
            tracing::info!("message inside span");
        });

        // Assert -- file buffer must not contain ANSI escape sequences
        let file_output = file_buf.lock().unwrap();
        let file_str = String::from_utf8_lossy(&file_output);

        assert!(
            !file_output.is_empty(),
            "File layer should have captured output"
        );
        assert!(
            !contains_ansi(&file_output),
            "File layer output must not contain ANSI escape sequences.\n\
             File output was:\n{file_str}"
        );

        // The file output should contain the span field value in plain text
        assert!(
            file_str.contains("some_field"),
            "File layer output should contain the span field name 'some_field'.\n\
             File output was:\n{file_str}"
        );

        // Assert -- console buffer SHOULD contain ANSI (confirms both layers work)
        let console_output = console_buf.lock().unwrap();
        assert!(
            !console_output.is_empty(),
            "Console layer should have captured output"
        );
        assert!(
            contains_ansi(&console_output),
            "Console layer output should contain ANSI escape sequences, \
             confirming the console layer is ANSI-enabled."
        );
    }

    // ==================== Test helpers ====================

    /// A writer that appends to a shared `Vec<u8>` buffer.
    /// Implements `std::io::Write` so it can be used with `fmt::layer().with_writer()`.
    struct MockWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for MockWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut lock = self.0.lock().unwrap();
            lock.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
