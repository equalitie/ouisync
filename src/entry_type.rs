use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Type of filesystem entry.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Deserialize, Serialize)]
pub enum EntryType {
    File,
    Directory,
}

impl EntryType {
    pub fn check_is_file(&self) -> Result<()> {
        match self {
            EntryType::File => Ok(()),
            EntryType::Directory => Err(Error::EntryIsDirectory),
        }
    }

    pub fn check_is_directory(&self) -> Result<()> {
        match self {
            EntryType::Directory => Ok(()),
            _ => Err(Error::EntryNotDirectory),
        }
    }
}
