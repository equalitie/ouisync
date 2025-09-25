use ansi_term::{Color, Style};
use std::fmt::Result as FmtResult;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{
    fmt::{
        format::Writer, time::FormatTime, FmtContext, FormatEvent, FormatFields, FormattedFields,
    },
    registry::LookupSpan,
};

pub struct Formatter<T> {
    timer: Option<T>,
    level: bool,
}

impl Default for Formatter<()> {
    fn default() -> Self {
        Self {
            timer: None,
            level: true,
        }
    }
}

impl<T> Formatter<T>
where
    T: FormatTime,
{
    pub fn with_timer<U>(self, timer: U) -> Formatter<U> {
        Formatter {
            timer: Some(timer),
            level: self.level,
        }
    }

    pub fn with_level(self, enabled: bool) -> Self {
        Self {
            level: enabled,
            ..self
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
        if let Some(timer) = &self.timer {
            format_timestamp(timer, &mut writer)?;
        }

        // Level
        if self.level {
            format_level(*metadata.level(), &mut writer)?;
        }

        // Fields
        ctx.format_fields(writer.by_ref(), event)?;

        // Spans
        if let Some(scope) = ctx.event_scope() {
            write!(writer, "{}", dimmed.paint(SEPARATOR))?;

            let mut seen = false;

            for span in scope {
                if seen {
                    write!(writer, "{}", dimmed.paint(":"))?;
                }

                seen = true;

                write!(writer, "{}", span.metadata().name())?;

                let ext = span.extensions();
                if let Some(fields) = &ext.get::<FormattedFields<N>>()
                    && !fields.is_empty()
                {
                    write!(writer, "{{{fields}}}")?;
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

fn format_timestamp<T>(timer: &T, writer: &mut Writer<'_>) -> FmtResult
where
    T: FormatTime,
{
    if writer.has_ansi_escapes() {
        let style = Style::new().dimmed();
        write!(writer, "{}", style.prefix())?;

        if timer.format_time(writer).is_err() {
            writer.write_str("<unknown time>")?;
        }

        write!(writer, "{} ", style.suffix())?;

        Ok(())
    } else {
        if timer.format_time(writer).is_err() {
            writer.write_str("<unknown time>")?;
        }
        writer.write_char(' ')
    }
}
