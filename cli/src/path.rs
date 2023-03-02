use std::{
    ffi::OsString,
    fmt,
    ops::Deref,
    path::{self, Path},
};

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

impl Deref for PathBuf {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
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
