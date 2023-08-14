use file_rotate::{compression::Compression, suffix::AppendCount, ContentLimit, FileRotate};
use std::path::Path;
use tracing_subscriber::EnvFilter;

pub(super) fn create_log_filter() -> EnvFilter {
    EnvFilter::builder()
        // TODO: Allow changing the log level at runtime or at least at init
        // time (via a command-line option or so)
        .with_default_directive("ouisync=debug".parse().unwrap())
        .from_env_lossy()
}

pub(super) fn create_file_writer(path: &Path) -> FileRotate<AppendCount> {
    FileRotate::new(
        path,
        AppendCount::new(1),
        ContentLimit::BytesSurpassed(10 * 1024 * 1024),
        Compression::None,
        #[cfg(unix)]
        None,
    )
}
