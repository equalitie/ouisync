use std::path::Path;
use tokio::{sync::mpsc, task};
use tokio_stream::wrappers::ReceiverStream;
use walkdir::{DirEntry, Error, WalkDir};

/// Returns a stream that recursively walks a directory. This is an async version of `WalkDir`.
pub(crate) fn walk_dir(root: impl AsRef<Path>) -> ReceiverStream<Result<DirEntry, Error>> {
    let root = root.as_ref().to_owned();

    let (tx, rx) = mpsc::channel(1);
    let rx = ReceiverStream::new(rx);

    task::spawn_blocking(move || {
        for entry in WalkDir::new(root) {
            if tx.blocking_send(entry).is_err() {
                break;
            }
        }
    });

    rx
}
