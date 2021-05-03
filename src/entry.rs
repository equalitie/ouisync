use crate::{directory::Directory, error::Error, file::File, locator::Locator};
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;

/// Type of filesystem entry.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Deserialize, Serialize)]
pub enum EntryType {
    File,
    Directory,
}

impl EntryType {
    pub fn check_is_directory(&self) -> Result<(), EntryNotDirectory> {
        match self {
            EntryType::Directory => Ok(()),
            _ => Err(EntryNotDirectory),
        }
    }
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

    pub fn locator(&self) -> &Locator {
        match self {
            Self::File(file) => file.locator(),
            Self::Directory(dir) => dir.locator(),
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

impl TryFrom<Entry> for Directory {
    type Error = EntryNotDirectory;

    fn try_from(entry: Entry) -> Result<Self, Self::Error> {
        match entry {
            Entry::Directory(dir) => Ok(dir),
            _ => Err(EntryNotDirectory),
        }
    }
}

pub struct EntryNotDirectory;

impl From<EntryNotDirectory> for Error {
    fn from(_: EntryNotDirectory) -> Self {
        Self::EntryNotDirectory
    }
}
