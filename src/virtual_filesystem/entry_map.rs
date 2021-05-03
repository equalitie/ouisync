use ouisync::{Directory, Entry, Error, Result};
use slab::Slab;
use std::convert::TryInto;

pub type FileHandle = u64;

// TODO: create separate maps for files and directories

#[derive(Default)]
pub struct EntryMap(Slab<Entry>);

impl EntryMap {
    pub fn insert(&mut self, entry: Entry) -> FileHandle {
        index_to_handle(self.0.insert(entry))
    }

    pub fn remove(&mut self, handle: FileHandle) -> Entry {
        self.0.remove(handle_to_index(handle))
    }

    pub fn get(&self, handle: FileHandle) -> Result<&Entry> {
        self.0
            .get(handle_to_index(handle))
            .ok_or(Error::EntryNotFound)
    }

    pub fn get_mut(&mut self, handle: FileHandle) -> Result<&mut Entry> {
        self.0
            .get_mut(handle_to_index(handle))
            .ok_or(Error::EntryNotFound)
    }

    pub fn get_directory(&self, handle: FileHandle) -> Result<&Directory> {
        match self.get(handle) {
            Ok(Entry::Directory(dir)) => Ok(dir),
            Ok(_) => Err(Error::EntryNotDirectory),
            Err(error) => Err(error),
        }
    }

    pub fn get_directory_mut(&mut self, handle: FileHandle) -> Result<&mut Directory> {
        match self.get_mut(handle) {
            Ok(Entry::Directory(dir)) => Ok(dir),
            Ok(_) => Err(Error::EntryNotDirectory),
            Err(error) => Err(error),
        }
    }
}

fn index_to_handle(index: usize) -> FileHandle {
    // `usize` to `u64` should never fail but for some reason there is no `impl From<usize> for u64`.
    index.try_into().unwrap_or_else(|_| unreachable!())
}

fn handle_to_index(handle: FileHandle) -> usize {
    handle.try_into().expect("file handle out of bounds")
}
