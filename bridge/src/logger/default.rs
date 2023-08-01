use super::common::{self, Formatter};
use std::{io, path::Path, sync::Mutex};
use tracing_subscriber::{
    fmt::{self, time::SystemTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

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
            .event_format(Formatter::<SystemTime>::default())
            .with_ansi(colors);

        // Log to file
        let file_layer = log_path.map(|path| {
            fmt::layer()
                .event_format(Formatter::<SystemTime>::default())
                .with_ansi(true)
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
