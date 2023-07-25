use super::common;
use std::{io, path::Path};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub(super) struct Inner;

impl Inner {
    pub fn new(log_path: Option<&Path>) -> io::Result<Self> {
        // Disable colors in output on Windows as `cmd` doesn't seem to support it.
        #[cfg(target_os = "windows")]
        let colors = false;
        #[cfg(not(target_os = "windows"))]
        let colors = true;

        // Log to stdout
        let stdout_layer = fmt::layer()
            .pretty()
            .with_ansi(colors)
            .with_target(false)
            .with_file(true)
            .with_line_number(true);

        // Log to file
        let file_layer = log_path.map(common::create_file_log_layer);

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
