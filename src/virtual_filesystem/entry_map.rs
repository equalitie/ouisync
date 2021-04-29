use super::handle_generator::HandleGenerator;
use ouisync::Entry;
use std::collections::HashMap;

pub type FileHandle = u64;

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
        todo!()
    }
}
