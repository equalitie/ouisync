use fuser::FUSE_ROOT_ID;
use ouisync::Locator;
use std::{
    collections::{hash_map::Entry, HashMap},
    ffi::OsString,
};

/// Inode handle
pub type Inode = u64;

pub struct InodeMap {
    forward: HashMap<Inode, Data>,
    reverse: HashMap<Key, Inode>,
    next_inode: Inode,
}

impl InodeMap {
    pub fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
            next_inode: FUSE_ROOT_ID + 1,
        }
    }

    pub fn lookup(&mut self, parent: Inode, name: OsString, locator: Locator) -> Inode {
        let key = (parent, name);

        //
        let next_inode = &mut self.next_inode;
        let forward = &self.forward;

        let inode = *self.reverse.entry(key.clone()).or_insert_with(|| {
            let inode = find_available_inode(*next_inode, forward);
            *next_inode = next_inode.wrapping_add(1);
            inode
        });

        let data = self.forward.entry(inode).or_insert_with(|| Data {
            lookups: 0,
            locator,
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

    pub fn get(&self, inode: Inode) -> Option<Locator> {
        if inode == FUSE_ROOT_ID {
            Some(Locator::Root)
        } else {
            self.forward.get(&inode).map(|data| data.locator)
        }
    }
}

fn find_available_inode(mut candidate: Inode, taken: &HashMap<Inode, Data>) -> Inode {
    assert!((taken.len() as u64) < u64::MAX);

    loop {
        if candidate == FUSE_ROOT_ID || taken.contains_key(&candidate) {
            candidate = candidate.wrapping_add(1);
        } else {
            break;
        }
    }

    candidate
}

type Key = (Inode, OsString);

struct Data {
    lookups: u64,
    locator: Locator,
    key: Key,
}
