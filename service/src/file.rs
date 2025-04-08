use ouisync::{Branch, File};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use slab::Slab;

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
    files: Slab<FileHolder>,
}

impl FileSet {
    pub fn new() -> Self {
        Self { files: Slab::new() }
    }

    pub fn insert(&mut self, holder: FileHolder) -> FileHandle {
        FileHandle(self.files.insert(holder))
    }

    pub fn remove(&mut self, handle: FileHandle) -> Option<FileHolder> {
        self.files.try_remove(handle.0)
    }

    pub fn get(&self, handle: FileHandle) -> Option<&FileHolder> {
        self.files.get(handle.0)
    }

    pub fn get_mut(&mut self, handle: FileHandle) -> Option<&mut FileHolder> {
        self.files.get_mut(handle.0)
    }
}

pub(crate) struct FileHolder {
    pub file: File,
    pub local_branch: Branch,
}
