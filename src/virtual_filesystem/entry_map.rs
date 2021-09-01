use ouisync::{debug_printer::DebugPrinter, Error, File, JointDirectory, JointEntry, Result};
use slab::Slab;
use std::convert::TryInto;

pub type FileHandle = u64;

// TODO: create separate maps for files and directories

#[derive(Default)]
pub struct EntryMap(Slab<JointEntry>);

impl EntryMap {
    pub fn insert(&mut self, entry: JointEntry) -> FileHandle {
        index_to_handle(self.0.insert(entry))
    }

    pub fn remove(&mut self, handle: FileHandle) -> JointEntry {
        self.0.remove(handle_to_index(handle))
    }

    pub fn get_file_mut(&mut self, handle: FileHandle) -> Result<&mut File> {
        self.get_mut(handle)?.as_file_mut()
    }

    pub fn get_directory(&self, handle: FileHandle) -> Result<&JointDirectory> {
        self.get(handle)?.as_directory()
    }

    pub fn get_directory_mut(&mut self, handle: FileHandle) -> Result<&mut JointDirectory> {
        self.get_mut(handle)?.as_directory_mut()
    }

    fn get(&self, handle: FileHandle) -> Result<&JointEntry> {
        self.0
            .get(handle_to_index(handle))
            .ok_or(Error::EntryNotFound)
    }

    fn get_mut(&mut self, handle: FileHandle) -> Result<&mut JointEntry> {
        self.0
            .get_mut(handle_to_index(handle))
            .ok_or(Error::EntryNotFound)
    }

    // For debugging, use when needed
    #[allow(dead_code)]
    pub async fn debug_print(&self, print: DebugPrinter) {
        print.display(&"EntryMap");
        let print = print.indent();
        for (i, entry) in &self.0 {
            let h = index_to_handle(i);
            match entry {
                JointEntry::File(file) => {
                    print.display(&format!("{}: {:?}", h, file));
                }
                JointEntry::Directory(dir) => {
                    print.display(&format!("{}: {:?}", h, dir));
                }
            }
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
