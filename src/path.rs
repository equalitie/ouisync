//! Utilities for working with filesystem paths.

use camino::Utf8Path;

/// Extension methods for the `Utf8Path` type.
pub(crate) trait Utf8Ext {
    /// Decomposes `path` into parent and filename. Returns `None` if `path` doesn't have parent
    /// (it's the root).
    fn decompose(&self) -> Option<(&Self, &str)>;
}

impl Utf8Ext for Utf8Path {
    fn decompose(&self) -> Option<(&Self, &str)> {
        match (self.parent(), self.file_name()) {
            (Some(parent), Some(name)) => Some((parent, name)),
            _ => None,
        }
    }
}
