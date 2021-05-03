use fuser::FUSE_ROOT_ID;
use ouisync::{EntryType, Locator};
use slab::Slab;
use std::{
    collections::{hash_map::Entry, HashMap},
    convert::TryInto,
    ffi::{OsStr, OsString},
};

/// Inode handle
pub type Inode = u64;

pub struct InodeMap {
    forward: Slab<InodeData>,
    reverse: HashMap<Key, Inode>,
}

impl InodeMap {
    pub fn new() -> Self {
        // Create inode for the root directory
        let mut forward = Slab::with_capacity(1);

        let index = forward.insert(InodeData {
            details: InodeDetails {
                locator: Locator::Root,
                entry_type: EntryType::Directory,
                parent: 0,
            },
            name: OsString::new(),
            lookups: 1,
        });
        let inode = index_to_inode(index);
        assert_eq!(inode, FUSE_ROOT_ID);

        Self {
            forward,
            reverse: HashMap::new(),
        }
    }

    // Lookup the inode for an entry.
    //
    // # Panics
    //
    // Panics if the parent inode doesn't exists.
    pub fn lookup(
        &mut self,
        parent: Inode,
        name: &OsStr,
        locator: Locator,
        entry_type: EntryType,
    ) -> Inode {
        // TODO: consider using `Arc` to avoid the double clone of `name`.

        let key = Key {
            parent,
            name: name.to_owned(),
        };

        match self.reverse.entry(key) {
            Entry::Vacant(entry) => {
                let index = self.forward.insert(InodeData {
                    details: InodeDetails {
                        locator,
                        entry_type,
                        parent,
                    },
                    name: name.to_owned(),
                    lookups: 1,
                });
                let inode = index_to_inode(index);

                entry.insert(inode);

                inode
            }
            Entry::Occupied(entry) => {
                let inode = *entry.get();
                let index = inode_to_index(inode);

                let data = &mut self.forward[index];
                data.lookups = data.lookups.checked_add(1).expect("too many inode lookups");

                inode
            }
        }
    }

    // Forget the given number of lookups of the given inode. If the number of lookups drops to
    // zero, the inode is removed.
    pub fn forget(&mut self, inode: Inode, lookups: u64) {
        let index = inode_to_index(inode);
        let data = &mut self.forward[index];

        if data.lookups <= lookups {
            let data = self.forward.remove(index);
            let key = Key {
                parent: data.details.parent,
                name: data.name,
            };

            self.reverse.remove(&key);
        } else {
            data.lookups -= lookups;
        }
    }

    // Retrieve the details for the given inode.
    //
    // # Panics
    //
    // Panics if the inode doesn't exists.
    pub fn get(&self, inode: Inode) -> &InodeDetails {
        self.forward
            .get(inode_to_index(inode))
            .map(|data| &data.details)
            .expect("inode not found")
    }
}

#[derive(Clone, Copy)]
pub struct InodeDetails {
    pub locator: Locator,
    pub entry_type: EntryType,
    pub parent: Inode,
}

struct InodeData {
    details: InodeDetails,
    name: OsString,
    lookups: u64,
}

#[derive(Eq, PartialEq, Hash)]
struct Key {
    parent: Inode,
    name: OsString,
}

fn index_to_inode(index: usize) -> Inode {
    // Map inode 1 to index 0 because 1 is the first valid inode (corresponding to the root
    // directory)
    index
        .wrapping_add(1)
        .try_into()
        .unwrap_or_else(|_| unreachable!())
}

fn inode_to_index(inode: Inode) -> usize {
    inode
        .wrapping_sub(1)
        .try_into()
        .expect("inode out of bounds")
}
