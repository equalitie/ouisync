#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "redirect.rs"]
mod inner;

#[cfg(target_os = "android")]
#[path = "android.rs"]
mod inner;

// TODO: ios and macos

#[cfg(test)]
mod tests;

use std::{io, path::Path};

use file_rotate::{compression::Compression, suffix::AppendCount, ContentLimit, FileRotate};
use inner::Inner;

/// Utility to capture the log messages produced by the running program and write them to the
/// specified file.
pub struct Logtee {
    inner: Option<Inner>,
}

impl Logtee {
    pub fn new(path: impl AsRef<Path>, options: RotateOptions) -> io::Result<Self> {
        let file = FileRotate::new(
            path,
            AppendCount::new(options.max_files),
            ContentLimit::Bytes(options.max_file_size),
            Compression::None,
            None,
        );

        Ok(Self {
            inner: Some(Inner::new(file)?),
        })
    }

    pub fn close(mut self) -> io::Result<()> {
        self.inner
            .take()
            .map(|inner| inner.close())
            .unwrap_or(Ok(()))
    }
}

impl Drop for Logtee {
    fn drop(&mut self) {
        self.inner.take().map(|inner| inner.close());
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RotateOptions {
    /// Max size in bytes of the log file before is rotated.
    pub max_file_size: usize,
    /// Max number of rotated files (i.e. excluding the main file) to keep.
    pub max_files: usize,
}

impl Default for RotateOptions {
    fn default() -> Self {
        Self {
            max_file_size: 1024 * 1024,
            max_files: 1,
        }
    }
}
