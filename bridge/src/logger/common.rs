use ansi_term::{Color, Style};
use file_rotate::{compression::Compression, suffix::AppendCount, ContentLimit, FileRotate};
use std::{fmt::Result as FmtResult, path::Path};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{
    fmt::{
        format::Writer, time::FormatTime, FmtContext, FormatEvent, FormatFields, FormattedFields,
    },
    registry::LookupSpan,
    EnvFilter,
};

pub(super) fn create_log_filter() -> EnvFilter {
    EnvFilter::builder()
        // TODO: Allow changing the log level at runtime or at least at init
        // time (via a command-line option or so)
        .with_default_directive("ouisync=debug".parse().unwrap())
        .from_env_lossy()
}

pub(super) fn create_file_writer(path: &Path) -> FileRotate<AppendCount> {
    FileRotate::new(
        path,
        AppendCount::new(1),
        ContentLimit::BytesSurpassed(10 * 1024 * 1024),
        Compression::None,
        #[cfg(unix)]
        None,
    )
}

#[derive(Default)]
pub(super) struct Formatter<T> {
    timer: T,
}

impl<T> Formatter<T>
where
    T: FormatTime,
{
    fn format_timestamp(&self, writer: &mut Writer<'_>) -> FmtResult {
        if writer.has_ansi_escapes() {
            let style = Style::new().dimmed();
            write!(writer, "{}", style.prefix())?;

            if self.timer.format_time(writer).is_err() {
                writer.write_str("<unknown time>")?;
            }

            write!(writer, "{} ", style.suffix())?;

            Ok(())
        } else {
            if self.timer.format_time(writer).is_err() {
                writer.write_str("<unknown time>")?;
            }
            writer.write_char(' ')
        }
    }
}

impl<T, S, N> FormatEvent<S, N> for Formatter<T>
where
    T: FormatTime,
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> FmtResult {
        let metadata = event.metadata();

        let dimmed = if writer.has_ansi_escapes() {
            Style::new().dimmed()
        } else {
            Style::new()
        };

        const SEPARATOR: &str = " :: ";

        // Timestamp
        self.format_timestamp(&mut writer)?;

        // Level
        format_level(*metadata.level(), &mut writer)?;

        // Fields
        ctx.format_fields(writer.by_ref(), event)?;

        // Spans
        if let Some(scope) = ctx.event_scope() {
            write!(writer, "{}", dimmed.paint(SEPARATOR))?;

            let mut seen = false;

            for span in scope.from_root() {
                if seen {
                    write!(writer, "{}", dimmed.paint(":"))?;
                }

                seen = true;

                write!(writer, "{}", span.metadata().name())?;

                let ext = span.extensions();
                if let Some(fields) = &ext.get::<FormattedFields<N>>() {
                    if !fields.is_empty() {
                        write!(writer, "{{{}}}", fields)?;
                    }
                }
            }
        }

        // File + line number:
        if metadata.file().is_some() || metadata.line().is_some() {
            write!(writer, "{}", dimmed.paint(SEPARATOR))?;
        }

        if let Some(filename) = metadata.file() {
            write!(writer, "{}{}", dimmed.paint(filename), dimmed.paint(":"))?;
        }

        if let Some(line_number) = metadata.line() {
            write!(
                writer,
                "{}{}{}",
                dimmed.prefix(),
                line_number,
                dimmed.suffix(),
            )?;
        }

        writeln!(writer)
    }
}

fn level_style(level: Level) -> Style {
    match level {
        Level::TRACE => Color::Purple,
        Level::DEBUG => Color::Blue,
        Level::INFO => Color::Green,
        Level::WARN => Color::Yellow,
        Level::ERROR => Color::Red,
    }
    .into()
}

fn format_level(level: Level, writer: &mut Writer<'_>) -> FmtResult {
    let style = if writer.has_ansi_escapes() {
        level_style(level)
    } else {
        Style::new()
    };

    match level {
        Level::TRACE => write!(writer, "{} ", style.paint("TRACE")),
        Level::DEBUG => write!(writer, "{} ", style.paint("DEBUG")),
        Level::INFO => write!(writer, "{}  ", style.paint("INFO")),
        Level::WARN => write!(writer, "{}  ", style.paint("WARN")),
        Level::ERROR => write!(writer, "{} ", style.paint("ERROR")),
    }
}
