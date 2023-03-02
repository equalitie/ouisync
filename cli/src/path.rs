use std::{ffi::OsString, fmt, path};

/// Wrapper for `PathBuf` that implements `Display`
#[derive(Clone)]
pub(crate) struct PathBuf(path::PathBuf);

impl From<path::PathBuf> for PathBuf {
    fn from(inner: path::PathBuf) -> Self {
        Self(inner)
    }
}

impl From<PathBuf> for path::PathBuf {
    fn from(outer: PathBuf) -> Self {
        outer.0
    }
}

impl From<OsString> for PathBuf {
    fn from(os: OsString) -> Self {
        Self(os.into())
    }
}

impl fmt::Display for PathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.display(), f)
    }
}

impl fmt::Debug for PathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}
