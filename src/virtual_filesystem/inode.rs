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
        let generator = &mut self.generator;
        let forward = &self.forward;

        let inode = *self
            .reverse
            .entry((parent, name.clone()))
            .or_insert_with(|| {
                generator.next(|inode| {
                    inode != 0 && inode != FUSE_ROOT_ID && !forward.contains_key(&inode)
                })
            });

        let data = self.forward.entry(inode).or_insert_with(|| Data {
            details: InodeDetails {
                locator,
                entry_type,
                parent,
            },
            lookups: 0,
            name,
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
            let data = entry.remove();
            self.reverse.remove(&(data.details.parent, data.name));
        }
    }

    pub fn get(&self, inode: Inode) -> Result<&InodeDetails> {
        if inode == FUSE_ROOT_ID {
            Ok(&ROOT)
        } else {
            self.forward
                .get(&inode)
                .map(|data| &data.details)
                .ok_or(Error::EntryNotFound)
        }
    }
}

#[derive(Clone, Copy)]
pub struct InodeDetails {
    pub locator: Locator,
    pub entry_type: EntryType,
    pub parent: Inode,
}

const ROOT: InodeDetails = InodeDetails {
    locator: Locator::Root,
    entry_type: EntryType::Directory,
    parent: 0,
};

type Key = (Inode, OsString);

struct Data {
    details: InodeDetails,
    lookups: u64,
    name: OsString,
}
