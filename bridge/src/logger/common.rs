use file_rotate::{compression::Compression, suffix::AppendCount, ContentLimit, FileRotate};
use std::{path::Path, sync::Mutex};
use tracing_subscriber::{
    fmt::{
        self,
        format::{Format, Pretty},
        Layer,
    },
    EnvFilter,
};

pub(super) fn create_log_filter() -> EnvFilter {
    EnvFilter::builder()
        // TODO: Allow changing the log level at runtime or at least at init
        // time (via a command-line option or so)
        .with_default_directive("ouisync=debug".parse().unwrap())
        .from_env_lossy()
}

pub(super) fn create_file_log_layer<S>(
    path: &Path,
) -> Layer<S, Pretty, Format<Pretty>, Mutex<FileRotate<AppendCount>>> {
    let rotate = FileRotate::new(
        path,
        AppendCount::new(1),
        ContentLimit::BytesSurpassed(10 * 1024 * 1024),
        Compression::None,
        #[cfg(unix)]
        None,
    );

    fmt::layer()
        .pretty()
        .with_ansi(true)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_writer(Mutex::new(rotate))
}
