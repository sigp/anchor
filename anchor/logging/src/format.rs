//! Anchor logging format
//!
//! This file contains the logging formatting options used to display anchor logs.

use std::fmt;

use nu_ansi_term::{Color, Style};
use tracing_core::{Event, Level, Subscriber};
use tracing_subscriber::{
    fmt::{
        FmtContext, FormattedFields,
        format::{FormatEvent, FormatFields, Writer},
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
                    write!(writer, " {}", &fields.fields)?;
                }
            }
        }

        writeln!(writer)
    }
}
