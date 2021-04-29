use crate::{directory::Directory, file::File};
use serde::{Deserialize, Serialize};

/// Type of filesystem entry.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Deserialize, Serialize)]
pub enum EntryType {
    File,
    Directory,
}

/// Filesystem entry.
pub enum Entry {
    File(File),
    Directory(Directory),
}

#[allow(clippy::len_without_is_empty)]
impl Entry {
    pub fn entry_type(&self) -> EntryType {
        match self {
            Self::File(_) => EntryType::File,
            Self::Directory(_) => EntryType::Directory,
        }
    }

    /// Length of the entry in bytes.
    pub fn len(&self) -> u64 {
        match self {
            Self::File(file) => file.len(),
            Self::Directory(dir) => dir.len(),
        }
    }
}
