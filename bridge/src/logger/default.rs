use ouisync_lib::{StateMonitor, TracingLayer};
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
    pub(crate) fn new(state_monitor: Option<StateMonitor>) -> Result<Self, io::Error> {
        let tracing_layer = state_monitor.map(|state_monitor| {
            let tracing_layer = TracingLayer::new();
            tracing_layer.set_monitor(Some(state_monitor));
            tracing_layer
        });

        tracing_subscriber::registry()
            .with(
                tracing_layer.with_filter(
                    Targets::new()
                        // only events from the ouisync crates
                        // NOTE: We can further restrict this to just "ouisync::state_monitor" but
                        // for now we want also regular log events to be sent to the state monitor.
                        .with_target("ouisync", LevelFilter::TRACE),
                ),
            )
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
