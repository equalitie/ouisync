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
    pub(crate) fn new(trace_monitor: StateMonitor) -> Result<Self, io::Error> {
        let tracing_layer = TracingLayer::new();
        tracing_layer.set_monitor(Some(trace_monitor));

        tracing_subscriber::registry()
            .with(
                tracing_layer.with_filter(
                    Targets::new()
                        .with_target("ouisync", LevelFilter::TRACE)
                        // Disable traces from other ouisync_* crates (they can still be enabled
                        // in the next layer via RUST_LOG)
                        .with_target("ouisync_", LevelFilter::OFF),
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
            .try_init()
            // `Err` here just means the logger is already initialized, it's OK to ignore it.
            .unwrap_or(());

        Ok(Self)
    }
}
