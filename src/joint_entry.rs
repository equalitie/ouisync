use crate::{
    error::{Error, Result},
    file::File,
    joint_directory::JointDirectory,
};

/// Type of filesystem entry.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum JointEntryType {
    File,
    Directory,
}

#[derive(Debug)]
pub enum JointEntry {
    File(File),
    Directory(JointDirectory),
}

#[allow(clippy::len_without_is_empty)]
impl JointEntry {
    pub fn entry_type(&self) -> JointEntryType {
        match self {
            Self::File(_) => JointEntryType::File,
            Self::Directory(_) => JointEntryType::Directory,
        }
    }

    pub fn as_file_mut(&mut self) -> Result<&mut File> {
        match self {
            Self::File(file) => Ok(file),
            Self::Directory(_) => Err(Error::EntryIsDirectory),
        }
    }

    pub fn as_directory(&self) -> Result<&JointDirectory> {
        match self {
            Self::File(_) => Err(Error::EntryIsFile),
            Self::Directory(dirs) => Ok(dirs),
        }
    }

    pub fn as_directory_mut(&mut self) -> Result<&mut JointDirectory> {
        match self {
            Self::File(_) => Err(Error::EntryIsFile),
            Self::Directory(dirs) => Ok(dirs),
        }
    }

    /// Length of the entry in bytes.
    pub async fn len(&self) -> u64 {
        match self {
            Self::File(file) => file.len().await,
            Self::Directory(dir) => dir.read().await.len().await,
        }
    }
}
