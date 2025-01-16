#[cfg(target_os = "android")]
mod android;
mod callback;
mod color;
mod format;
#[cfg(target_os = "android")]
mod redirect;
#[cfg(not(target_os = "android"))]
mod stdout;

pub use callback::{BufferPool, Callback};
pub use color::{LogColor, ParseLogColorError};
pub use format::{LogFormat, ParseLogFormatError};

use file_rotate::{compression::Compression, suffix::AppendCount, ContentLimit, FileRotate};
use ouisync_tracing_fmt::Formatter;
use std::{
    env, fs, io,
    panic::{self, PanicHookInfo},
    path::Path,
    sync::Mutex,
};
use tracing::Subscriber;
use tracing_subscriber::{
    field::RecordFields,
    fmt::{
        format::{DefaultFields, Writer},
        time::SystemTime,
        FormatFields,
    },
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
    EnvFilter, Layer,
};

pub struct Builder<'a> {
    file: Option<&'a Path>,
    callback: Option<(Box<Callback>, BufferPool)>,
    format: LogFormat,
    color: LogColor,
    tag: &'a str,
}

impl<'a> Builder<'a> {
    pub fn with_file(self, path: &'a Path) -> Self {
        Self {
            file: Some(path),
            ..self
        }
    }

    pub fn with_callback(self, callback: Box<Callback>, pool: BufferPool) -> Self {
        Self {
            callback: Some((callback, pool)),
            ..self
        }
    }

    pub fn with_format(self, format: LogFormat) -> Self {
        Self { format, ..self }
    }

    pub fn with_color(self, color: LogColor) -> Self {
        Self { color, ..self }
    }

    pub fn with_tag(self, tag: &'a str) -> Self {
        Self { tag, ..self }
    }

    pub fn build(self) -> io::Result<Logger> {
        if let Some(parent) = self.file.and_then(|path| path.parent()) {
            fs::create_dir_all(parent)?;
        }

        // Log to file
        let file_layer = self.file.map(|path| {
            tracing_subscriber::fmt::layer()
                .event_format(Formatter::default().with_timer(SystemTime))
                .with_ansi(false)
                .with_writer(Mutex::new(create_file_writer(path)))
                // HACK: Workaround for https://github.com/tokio-rs/tracing/issues/1372. See
                // `TypedFields` for more detauls.
                .fmt_fields(TypedFields::default())
        });

        // Log by calling the callback
        let callback_layer = self
            .callback
            .map(|(callback, pool)| callback::layer(callback, pool));

        tracing_subscriber::registry()
            .with(create_log_filter())
            .with(default_layer(self.tag.to_owned(), self.format, self.color))
            .with(file_layer)
            .with(callback_layer)
            .try_init()
            // `Err` here just means the logger is already initialized, it's OK to ignore it.
            .unwrap_or(());

        // Log panics
        let default_panic_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            log_panic(panic_info);
            default_panic_hook(panic_info);
        }));

        Ok(Logger {
            #[cfg(target_os = "android")]
            _redirect: redirect::Redirect::new()?,
        })
    }
}

pub struct Logger {
    #[cfg(target_os = "android")]
    _redirect: redirect::Redirect,
}

impl Logger {
    pub fn builder<'a>() -> Builder<'a> {
        Builder {
            file: None,
            callback: None,
            format: LogFormat::Human,
            color: LogColor::Auto,
            tag: "",
        }
    }

    pub fn new() -> io::Result<Self> {
        Self::builder().build()
    }
}

fn create_log_filter() -> EnvFilter {
    EnvFilter::builder()
        // TODO: Allow changing the log level at runtime or at least at init
        // time (via a command-line option or so)
        .parse_lossy(
            env::var(EnvFilter::DEFAULT_ENV)
                .unwrap_or_else(|_| "ouisync=debug,deadlock=warn".to_string()),
        )
}

fn create_file_writer(path: &Path) -> FileRotate<AppendCount> {
    FileRotate::new(
        path,
        AppendCount::new(1),
        ContentLimit::BytesSurpassed(10 * 1024 * 1024),
        Compression::None,
        #[cfg(unix)]
        None,
    )
}

#[cfg(target_os = "android")]
fn default_layer<S>(tag: String, _format: LogFormat, _color: LogColor) -> impl Layer<S>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    android::layer(tag)
}

#[cfg(not(target_os = "android"))]
fn default_layer<S>(_tag: String, format: LogFormat, color: LogColor) -> impl Layer<S>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    stdout::layer(format, color)
}

fn log_panic(info: &PanicHookInfo) {
    match (
        info.payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str())),
        info.location(),
    ) {
        (Some(message), Some(location)) => tracing::error!(
            "panic '{}' at {}:{}:{}",
            message,
            location.file(),
            location.line(),
            location.column(),
        ),
        (Some(message), None) => tracing::error!("panic '{message}'"),
        (None, Some(location)) => tracing::error!(
            "panic at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        (None, None) => tracing::error!("panic"),
    };
}

// A newtype for `DefaultFields`. Needed to work around
// https://github.com/tokio-rs/tracing/issues/1372: By using a different type of `FormatFields` for
// each layer we force them to record the fields into their own span extension instead of all
// layers recording into the same extension. This avoid duplicating the fields in their respective
// outputs.
#[derive(Default)]
struct TypedFields(DefaultFields);

impl<'writer> FormatFields<'writer> for TypedFields {
    fn format_fields<R: RecordFields>(
        &self,
        writer: Writer<'writer>,
        fields: R,
    ) -> std::fmt::Result {
        self.0.format_fields(writer, fields)
    }
}
