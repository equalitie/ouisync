use std::{
    mem,
    ops::{Deref, DerefMut},
    sync::Mutex,
};

use ouisync::{Branch, File};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use slab::Slab;
use thiserror::Error;

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[serde(transparent)]
#[api]
pub struct FileHandle(usize);

impl FileHandle {
    #[cfg(test)]
    pub(crate) fn from_raw(raw: usize) -> Self {
        Self(raw)
    }
}

pub(crate) struct FileSet {
    files: Mutex<Slab<Lock>>,
}

impl FileSet {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(Slab::new()),
        }
    }

    pub fn insert(&self, holder: FileHolder) -> FileHandle {
        FileHandle(self.files.lock().unwrap().insert(Lock::Unlocked(holder)))
    }

    pub fn remove(&self, handle: FileHandle) -> Result<FileHolder, FileSetError> {
        let mut files = self.files.lock().unwrap();

        match files.get(handle.0) {
            Some(Lock::Unlocked(_)) => match files.try_remove(handle.0) {
                Some(Lock::Unlocked(holder)) => Ok(holder),
                Some(Lock::Locked) | None => unreachable!(),
            },
            Some(Lock::Locked) => Err(FileSetError::Locked),
            None => Err(FileSetError::NotFound),
        }
    }

    pub fn get(&self, handle: FileHandle) -> Result<FileGuard<'_>, FileSetError> {
        match mem::replace(
            self.files
                .lock()
                .unwrap()
                .get_mut(handle.0)
                .ok_or(FileSetError::NotFound)?,
            Lock::Locked,
        ) {
            Lock::Unlocked(holder) => Ok(FileGuard {
                handle,
                holder: Some(holder),
                files: &self.files,
            }),
            Lock::Locked => Err(FileSetError::Locked),
        }
    }
}

pub(crate) struct FileHolder {
    pub file: File,
    pub local_branch: Branch,
}

pub(crate) struct FileGuard<'a> {
    handle: FileHandle,
    holder: Option<FileHolder>,
    files: &'a Mutex<Slab<Lock>>,
}

impl Deref for FileGuard<'_> {
    type Target = FileHolder;

    fn deref(&self) -> &Self::Target {
        self.holder.as_ref().unwrap()
    }
}

impl DerefMut for FileGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.holder.as_mut().unwrap()
    }
}

impl Drop for FileGuard<'_> {
    fn drop(&mut self) {
        *self.files.lock().unwrap().get_mut(self.handle.0).unwrap() =
            Lock::Unlocked(self.holder.take().unwrap());
    }
}

#[derive(Debug, Error)]
pub(crate) enum FileSetError {
    #[error("file not found")]
    NotFound,
    #[error("file is locked")]
    Locked,
}

enum Lock {
    Unlocked(FileHolder),
    Locked,
}
