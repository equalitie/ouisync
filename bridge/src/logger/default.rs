use std::io;
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    fmt,
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Layer,
};

pub struct Logger;

impl Logger {
    pub(crate) fn new() -> Result<Self, io::Error> {
        // Disable colors in output on Windows as `cmd` doesn't seem to support it.
        #[cfg(target_os = "windows")]
        let colors = false;
        #[cfg(not(target_os = "windows"))]
        let colors = true;

        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .pretty()
                    .with_ansi(colors)
                    .with_target(false)
                    .with_file(true)
                    .with_line_number(true)
                    .with_filter(
                        EnvFilter::builder()
                            // TODO: Allow changing the log level at runtime or at least at init
                            // time (via a command-line option or so)
                            .with_default_directive(LevelFilter::DEBUG.into())
                            .from_env_lossy(),
                    ),
            )
            .with(
                // This is a global filter which is fast. Filters can also be set on the individual
                // layers, but those are slower. So we want to filter out as much as possible here.
                Targets::new()
                    // everything from the ouisync crates
                    .with_target("ouisync", LevelFilter::TRACE)
                    // DHT routing events
                    .with_target("btdht::routing", LevelFilter::DEBUG)
                    // warnings from sqlx (they warn about slow queries among other things)
                    .with_target("sqlx", LevelFilter::WARN),
            )
            .try_init()
            // `Err` here just means the logger is already initialized, it's OK to ignore it.
            .unwrap_or(());

        Ok(Self)
    }
}

pub struct CaptureOutput {}
