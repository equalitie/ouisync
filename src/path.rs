//! Utilities for working with filesystem paths.

/// Utilities for `std::path::Path`.
pub(crate) mod os {
    use std::{
        ffi::OsStr,
        path::{Path, PathBuf},
    };

    /// Append the given suffix to the end of this path. Note unlike `join`, this appends the
    /// suffix verbatim, without putting a path separator in between.
    pub fn append(path: &Path, suffix: impl AsRef<OsStr>) -> PathBuf {
        let mut buf = path.as_os_str().to_owned();
        buf.push(suffix);
        PathBuf::from(buf)
    }
}

/// Utilities for `Utf8Path`.
pub(crate) mod utf8 {
    use camino::Utf8Path;

    /// Decomposes `path` into parent and filename. Returns `None` if `path` doesn't have parent
    /// (it's the root).
    pub fn decompose(path: &Utf8Path) -> Option<(&Utf8Path, &str)> {
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => Some((parent, name)),
            _ => None,
        }
    }
}
