use super::handle_generator::HandleGenerator;
use fuser::FUSE_ROOT_ID;
use ouisync::{EntryType, Error, Locator, Result};
use std::{
    collections::{hash_map::Entry, HashMap},
    ffi::OsString,
};

/// Inode handle
pub type Inode = u64;

#[derive(Default)]
pub struct InodeMap {
    forward: HashMap<Inode, Data>,
    reverse: HashMap<Key, Inode>,
    generator: HandleGenerator,
}

impl InodeMap {
    pub fn lookup(
        &mut self,
        parent: Inode,
        name: OsString,
        locator: Locator,
        entry_type: EntryType,
    ) -> Inode {
        let key = (parent, name);

        //
        let generator = &mut self.generator;
        let forward = &self.forward;

        let inode = *self.reverse.entry(key.clone()).or_insert_with(|| {
            generator
                .next(|inode| inode != 0 && inode != FUSE_ROOT_ID && !forward.contains_key(&inode))
        });

        let data = self.forward.entry(inode).or_insert_with(|| Data {
            lookups: 0,
            locator,
            entry_type,
            key,
        });

        data.lookups = data.lookups.saturating_add(1);

        inode
    }

    pub fn forget(&mut self, inode: Inode, lookups: u64) {
        let mut entry = match self.forward.entry(inode) {
            Entry::Occupied(entry) => entry,
            Entry::Vacant(_) => return,
        };

        entry.get_mut().lookups = entry.get().lookups.saturating_sub(lookups);

        if entry.get().lookups == 0 {
            self.reverse.remove(&entry.get().key);
            entry.remove();
        }
    }

    pub fn get(&self, inode: Inode) -> Result<(Locator, EntryType)> {
        if inode == FUSE_ROOT_ID {
            Ok((Locator::Root, EntryType::Directory))
        } else {
            self.forward
                .get(&inode)
                .map(|data| (data.locator, data.entry_type))
                .ok_or(Error::EntryNotFound)
        }
    }

    pub fn get_directory(&self, inode: Inode) -> Result<Locator> {
        match self.get(inode) {
            Ok((locator, EntryType::Directory)) => Ok(locator),
            Ok(_) => Err(Error::EntryNotDirectory),
            Err(error) => Err(error),
        }
    }
}

type Key = (Inode, OsString);

struct Data {
    lookups: u64,
    locator: Locator,
    entry_type: EntryType,
    key: Key,
}
