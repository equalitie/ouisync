use super::{common, LogColor, LogFormat};
use ouisync_tracing_fmt::Formatter;
#[cfg(not(target_os = "windows"))]
use std::io::IsTerminal;
use std::{io, path::Path, sync::Mutex};
use tracing::{
    metadata::LevelFilter,
    span::{Attributes, Record},
    Event, Id, Metadata, Subscriber,
};
use tracing_subscriber::{
    fmt::{self, time::SystemTime},
    layer::{Context, Layer, SubscriberExt},
    util::SubscriberInitExt,
};

pub(super) struct Inner;

impl Inner {
    pub fn new(path: Option<&Path>, format: LogFormat, color: LogColor) -> io::Result<Self> {
        let color = match color {
            LogColor::Always => true,
            LogColor::Never => false,
            LogColor::Auto => {
                // Disable colors in output on Windows as `cmd` doesn't seem to support it.
                //
                // TODO: consider using `ansi_term::enable_ansi_support()`
                // (see https://github.com/ogham/rust-ansi-term#basic-usage for more info)
                !cfg!(target_os = "windows") && io::stdout().is_terminal()
            }
        };

        // Log to stdout
        let stdout_layer = match format {
            LogFormat::Human => EitherLayer::A(
                fmt::layer()
                    .event_format(Formatter::<SystemTime>::default())
                    .with_ansi(color),
            ),
            LogFormat::Json => EitherLayer::B(
                fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(false),
            ),
        };

        // Log to file
        let file_layer = path.map(|path| {
            fmt::layer()
                .event_format(Formatter::<SystemTime>::default())
                .with_ansi(false)
                .with_writer(Mutex::new(common::create_file_writer(path)))
        });

        tracing_subscriber::registry()
            .with(common::create_log_filter())
            .with(stdout_layer)
            .with(file_layer)
            .try_init()
            // `Err` here just means the logger is already initialized, it's OK to ignore it.
            .unwrap_or(());

        Ok(Self)
    }
}

enum EitherLayer<A, B> {
    A(A),
    B(B),
}

impl<A, B, S> Layer<S> for EitherLayer<A, B>
where
    A: Layer<S>,
    B: Layer<S>,
    S: Subscriber,
{
    fn on_register_dispatch(&self, collector: &tracing::Dispatch) {
        match self {
            Self::A(l) => l.on_register_dispatch(collector),
            Self::B(l) => l.on_register_dispatch(collector),
        }
    }

    fn on_layer(&mut self, subscriber: &mut S) {
        match self {
            Self::A(l) => l.on_layer(subscriber),
            Self::B(l) => l.on_layer(subscriber),
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>, ctx: Context<'_, S>) -> bool {
        match self {
            Self::A(l) => l.enabled(metadata, ctx),
            Self::B(l) => l.enabled(metadata, ctx),
        }
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        match self {
            Self::A(l) => l.on_new_span(attrs, id, ctx),
            Self::B(l) => l.on_new_span(attrs, id, ctx),
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        match self {
            Self::A(l) => l.max_level_hint(),
            Self::B(l) => l.max_level_hint(),
        }
    }

    fn on_record(&self, span: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        match self {
            Self::A(l) => l.on_record(span, values, ctx),
            Self::B(l) => l.on_record(span, values, ctx),
        }
    }

    fn on_follows_from(&self, span: &Id, follows: &Id, ctx: Context<'_, S>) {
        match self {
            Self::A(l) => l.on_follows_from(span, follows, ctx),
            Self::B(l) => l.on_follows_from(span, follows, ctx),
        }
    }

    fn event_enabled(&self, event: &Event<'_>, ctx: Context<'_, S>) -> bool {
        match self {
            Self::A(l) => l.event_enabled(event, ctx),
            Self::B(l) => l.event_enabled(event, ctx),
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        match self {
            Self::A(l) => l.on_event(event, ctx),
            Self::B(l) => l.on_event(event, ctx),
        }
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        match self {
            Self::A(l) => l.on_enter(id, ctx),
            Self::B(l) => l.on_enter(id, ctx),
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        match self {
            Self::A(l) => l.on_exit(id, ctx),
            Self::B(l) => l.on_exit(id, ctx),
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        match self {
            Self::A(l) => l.on_close(id, ctx),
            Self::B(l) => l.on_close(id, ctx),
        }
    }

    fn on_id_change(&self, old: &Id, new: &Id, ctx: Context<'_, S>) {
        match self {
            Self::A(l) => l.on_id_change(old, new, ctx),
            Self::B(l) => l.on_id_change(old, new, ctx),
        }
    }
}
