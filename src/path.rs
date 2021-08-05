//! Utilities for working with filesystem paths.

use camino::Utf8Path;

/// Decomposes `path` into parent and filename. Returns `None` if `path` doesn't have parent
/// (it's the root).
pub fn decompose(path: &Utf8Path) -> Option<(&Utf8Path, &str)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Some((parent, name)),
        _ => None,
    }
}
