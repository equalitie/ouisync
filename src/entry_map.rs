use ouisync::Entry;
use std::collections::HashMap;

pub type FileHandle = u64;

pub struct EntryMap {
    map: HashMap<FileHandle, Entry>,
    next_handle: FileHandle,
}

impl EntryMap {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            next_handle: 0,
        }
    }

    pub fn insert(&mut self, entry: Entry) -> FileHandle {
        let handle = self.next_handle();
        self.map.insert(handle, entry);
        handle
    }

    pub fn remove(&mut self, handle: FileHandle) -> Option<Entry> {
        todo!()
    }

    fn next_handle(&mut self) -> FileHandle {
        let handle = (self.next_handle..=u64::MAX)
            .chain(0..self.next_handle)
            .find(|candidate| !self.map.contains_key(&candidate))
            .expect("all file handles taken");
        self.next_handle = handle.wrapping_add(1);
        handle
    }
}
