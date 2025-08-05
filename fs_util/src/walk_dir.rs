pub use walkdir::{DirEntry, Error as DirEntryError};

use futures_util::Stream;
use std::path::Path;
use tokio::{sync::mpsc, task};
use tokio_stream::wrappers::ReceiverStream;

/// Async version of [WalkDir](https://docs.rs/walkdir/2.5.0/walkdir/struct.WalkDir.html).
pub struct WalkDir {
    inner: walkdir::WalkDir,
}

impl WalkDir {
    /// Creates a builder for a recursive directory stream starting at `root`. Use [Self::into_stream] to
    /// convert it into the resulting stream.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            inner: walkdir::WalkDir::new(root),
        }
    }

    /// Yields a directory’s contents before the directory itself. By default, this is disabled.
    pub fn contents_first(self, yes: bool) -> Self {
        Self {
            inner: self.inner.contents_first(yes),
        }
    }

    /// Obtains the `Stream` that yields the entries.
    pub fn into_stream(self) -> impl Stream<Item = Result<DirEntry, DirEntryError>> {
        let Self { inner } = self;

        let (tx, rx) = mpsc::channel(1);
        let rx = ReceiverStream::new(rx);

        task::spawn_blocking(move || {
            for entry in inner {
                if tx.blocking_send(entry).is_err() {
                    break;
                }
            }
        });

        rx
    }
}
