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
        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .pretty()
                    .with_target(false)
                    .with_file(true)
                    .with_line_number(true)
                    .with_filter(
                        EnvFilter::builder()
                            // Only show the logs if explicitly enabled with the `RUST_LOG` env
                            // variable.
                            .with_default_directive(LevelFilter::OFF.into())
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
