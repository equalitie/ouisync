use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
    sync::{Arc, Mutex},
};

use ouisync_tracing_fmt::Formatter;
use tracing::Subscriber;
use tracing_subscriber::{
    Layer,
    fmt::{self, MakeWriter, time::SystemTime},
    registry::LookupSpan,
};

/// Layer that appends log events to the file at `path`, in addition to whatever other layer(s)
/// the subscriber already has (e.g. the stdout one). This exists because on iOS/macOS the
/// process' stdout isn't captured anywhere reliable once the app is backgrounded (and the
/// `logtee` library that would otherwise tee it into a file isn't built for those platforms), so
/// without this, diagnosing anything that happens after backgrounding is impossible.
pub(super) fn layer<S>(path: &Path) -> io::Result<impl Layer<S>>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new().create(true).append(true).open(path)?;

    Ok(fmt::layer()
        .event_format(Formatter::default().with_timer(SystemTime))
        .with_ansi(false)
        .with_writer(FileWriter(Arc::new(Mutex::new(file)))))
}

#[derive(Clone)]
struct FileWriter(Arc<Mutex<File>>);

impl io::Write for FileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

impl<'a> MakeWriter<'a> for FileWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn writes_events_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("ouisync.log");

        let subscriber = tracing_subscriber::registry().with(layer(&path).unwrap());

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("hello from the test");
        });

        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("hello from the test"),
            "log file contents: {contents:?}"
        );
    }
}
