use ouisync_lib::StateMonitor;
use std::{io, sync::Once};
use tracing::metadata::LevelFilter;
use tracing_subscriber::EnvFilter;

pub(crate) struct Logger;

impl Logger {
    pub fn new(_: StateMonitor) -> Result<Self, io::Error> {
        static LOG_INIT: Once = Once::new();
        LOG_INIT.call_once(|| {
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::builder()
                        // Only show the logs if explicitly enabled with the `RUST_LOG` env
                        // variable.
                        .with_default_directive(LevelFilter::OFF.into())
                        .from_env_lossy(),
                )
                .with_file(true)
                .with_line_number(true)
                .init()
        });

        Ok(Self)
    }
}
