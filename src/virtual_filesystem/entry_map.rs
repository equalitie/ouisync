use super::handle_generator::HandleGenerator;
use ouisync::{Directory, Entry, Error, Result};
use std::collections::HashMap;

pub type FileHandle = u64;

// TODO: create separate maps for files and directories

#[derive(Default)]
pub struct EntryMap {
    map: HashMap<FileHandle, Entry>,
    generator: HandleGenerator,
}

impl EntryMap {
    pub fn insert(&mut self, entry: Entry) -> FileHandle {
        let map = &self.map;
        let handle = self.generator.next(|handle| !map.contains_key(&handle));
        self.map.insert(handle, entry);
        handle
    }

    pub fn remove(&mut self, handle: FileHandle) -> Option<Entry> {
        self.map.remove(&handle)
    }

    pub fn get(&self, handle: FileHandle) -> Result<&Entry> {
        self.map.get(&handle).ok_or(Error::EntryNotFound)
    }

    pub fn get_directory(&self, handle: FileHandle) -> Result<&Directory> {
        match self.get(handle) {
            Ok(Entry::Directory(dir)) => Ok(dir),
            Ok(_) => Err(Error::EntryNotDirectory),
            Err(error) => Err(error),
        }
    }
}
