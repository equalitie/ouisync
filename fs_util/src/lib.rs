use futures_util::Stream;
use std::{io, path::Path};
use tokio::{
    fs::{self, File},
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
    task,
};
use tokio_stream::wrappers::ReceiverStream;

pub use walkdir::{DirEntry, Error as DirEntryError};

/// Async version of [WalkDir](https://docs.rs/walkdir/2.5.0/walkdir/struct.WalkDir.html).
pub struct WalkDir {
    inner: walkdir::WalkDir,
}

impl WalkDir {
    /// Creates a builder for a recursive directory stream starting at `root`. Use [Self::into_stream] to
    /// obtain the resulting stream.
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

/// Moves file from `src` to `dst`. If they are on the same filesystem, it does a simple rename.
/// Otherwise it copies `src` to `dst` first and then deletes `src`.
pub async fn move_file(src: &Path, dst: &Path) -> io::Result<()> {
    // First try rename
    match fs::rename(src, dst).await {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => (),
        Err(error) => return Err(error),
    }

    // `src` and `dst` are on different filesystems, fall back to copy + remove.
    fs::copy(src, dst).await?;
    fs::remove_file(src).await?;

    Ok(())
}

/// Check whether the two files have the same content.
pub async fn same_content(lhs: &mut File, rhs: &mut File) -> io::Result<bool> {
    if lhs.metadata().await?.len() != rhs.metadata().await?.len() {
        return Ok(false);
    }

    let mut lhs = BufReader::new(lhs);
    let mut rhs = BufReader::new(rhs);

    loop {
        let lhs_buf = lhs.fill_buf().await?;
        let rhs_buf = rhs.fill_buf().await?;

        match (lhs_buf.is_empty(), rhs_buf.is_empty()) {
            (true, true) => return Ok(true),
            (true, false) | (false, true) => return Ok(false),
            (false, false) => (),
        }

        let n = lhs_buf.len().min(rhs_buf.len());

        if lhs_buf[..n] != rhs_buf[..n] {
            return Ok(false);
        }

        lhs.consume(n);
        rhs.consume(n);
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn same_content_empty() {
        same_content_case(&[], &[], true).await;
    }

    #[tokio::test]
    async fn same_content_small_identical() {
        same_content_case(b"hello world", b"hello world", true).await;
    }

    #[tokio::test]
    async fn same_content_small_different_size() {
        same_content_case(b"xxx", b"yyyy", false).await;
    }

    #[tokio::test]
    async fn same_content_small_same_size_different_content() {
        same_content_case(b"xxxx", b"yyyy", false).await;
    }

    #[tokio::test]
    async fn same_content_large_identical() {
        let content = bytes(0, 8 * 1024 * 1024);
        same_content_case(&content, &content, true).await;
    }

    #[tokio::test]
    async fn same_content_large_different_size() {
        let content_a = bytes(0, 8 * 1024 * 1024);
        let content_b = bytes(0, 8 * 1024 * 1024 + 1);
        same_content_case(&content_a, &content_b, false).await;
    }

    #[tokio::test]
    async fn same_content_large_same_size_different_content() {
        let content_a = bytes(0, 8 * 1024 * 1024);
        let content_b = bytes(1, 8 * 1024 * 1024);
        same_content_case(&content_a, &content_b, false).await;
    }

    #[tokio::test]
    async fn same_content_large_same_size_almost_identical_content() {
        let content_a = bytes(0, 8 * 1024 * 1024);
        let content_b = {
            let mut v = bytes(0, 8 * 1024 * 1024);
            *v.last_mut().unwrap() = 0;
            v
        };

        same_content_case(&content_a, &content_b, false).await;
    }

    async fn same_content_case(lhs_content: &[u8], rhs_content: &[u8], expected: bool) {
        let temp_dir = TempDir::new().unwrap();
        let lhs_path = temp_dir.path().join("lhs");
        let rhs_path = temp_dir.path().join("rhs");

        fs::write(&lhs_path, lhs_content).await.unwrap();
        fs::write(&rhs_path, rhs_content).await.unwrap();

        let mut lhs = File::open(lhs_path).await.unwrap();
        let mut rhs = File::open(rhs_path).await.unwrap();

        let actual = same_content(&mut lhs, &mut rhs).await.unwrap();

        assert_eq!(actual, expected);
    }

    fn bytes(first: u8, count: usize) -> Vec<u8> {
        (first as usize..first as usize + count)
            .map(|n| (n % (u8::MAX as usize + 1)) as u8)
            .collect()
    }
}
