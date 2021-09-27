//! Utilities for working with filesystem paths.

use camino::Utf8Path;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Extension methods for the `Path` type.
pub(crate) trait PathExt {
    /// Append the given suffix to the end of this path. Note unlike `join`, this appends the
    /// suffix verbatim, without putting a path separator in between.
    fn append(&self, suffix: impl AsRef<OsStr>) -> PathBuf;
}

impl PathExt for Path {
    fn append(&self, suffix: impl AsRef<OsStr>) -> PathBuf {
        let mut buf = self.as_os_str().to_owned();
        buf.push(suffix);
        PathBuf::from(buf)
    }
}

/// Extension methods for the `Utf8Path` type.
pub(crate) trait Utf8PathExt {
    /// Decomposes `path` into parent and filename. Returns `None` if `path` doesn't have parent
    /// (it's the root).
    fn decompose(&self) -> Option<(&Self, &str)>;
}

impl Utf8PathExt for Utf8Path {
    fn decompose(&self) -> Option<(&Self, &str)> {
        match (self.parent(), self.file_name()) {
            (Some(parent), Some(name)) => Some((parent, name)),
            _ => None,
        }
    }
}
