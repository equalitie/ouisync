use camino::Utf8PathBuf;
use fuser::FUSE_ROOT_ID;
use ouisync::{Error, GlobalLocator, Result};
use slab::Slab;
use std::{
    collections::{hash_map::Entry, HashMap},
    convert::TryInto,
    fmt,
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
            representation: Representation::Directory,
            parent: 0,
            name: String::new(),
            lookups: 1,
        });
        let inode = index_to_inode(index);
        assert_eq!(inode, FUSE_ROOT_ID);

        log::trace!("Create inode {} for /", FUSE_ROOT_ID);

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
    pub fn lookup(&mut self, parent: Inode, name: &str, representation: Representation) -> Inode {
        // TODO: consider using `Arc` to avoid the double clone of `name`.

        let key = Key {
            parent,
            name: name.to_owned(),
        };

        match self.reverse.entry(key) {
            Entry::Vacant(entry) => {
                let index = self.forward.insert(InodeData {
                    representation,
                    parent,
                    name: name.to_owned(),
                    lookups: 1,
                });
                let inode = index_to_inode(index);

                entry.insert(inode);

                log::trace!(
                    "Create inode {} for {}",
                    inode,
                    PathDisplay(&self.forward, inode, None)
                );

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
                parent: data.parent,
                name: data.name,
            };

            self.reverse.remove(&key);

            log::trace!(
                "Remove inode {} for {}",
                inode,
                PathDisplay(&self.forward, key.parent, Some(&key.name))
            );
        } else {
            data.lookups -= lookups;
        }
    }

    // Retrieve the data for the given inode.
    //
    // # Panics
    //
    // Panics if the inode doesn't exist.
    pub fn get(&self, inode: Inode) -> InodeView {
        self.forward
            .get(inode_to_index(inode))
            .map(|data| InodeView { inodes: self, data })
            .expect("inode not found")
    }

    // Returns an object that displays the absolute (from the repository root) path of a given
    // inode. If `last` is `Some`, it is appended as the final component of the path. This is
    // useful for printing paths of non-existing entries.
    //
    // # Panics
    //
    // Panics if the inode doesn't exist.
    pub fn path_display<'a>(
        &'a self,
        inode: Inode,
        last: Option<&'a str>,
    ) -> impl fmt::Display + 'a {
        PathDisplay(&self.forward, inode, last)
    }

    fn calculate_path(&self, inode_data: &InodeData) -> Utf8PathBuf {
        if inode_data.parent == 0 {
            return Utf8PathBuf::new();
        }

        self.calculate_path(self.get(inode_data.parent).data)
            .join(&inode_data.name)
    }
}

pub enum Representation {
    // Because a single directory may be present in multiple branches, we can't simply store a
    // single locator to a directory. We could - in principle - store a set of locators here, but
    // then we would need to update the Representation each time a new branch with the given
    // directory is added. For now, we'll just store the path and each time the set of locators
    // corresponding to the path is requested, it'll be determined dynamically.
    Directory,
    File(GlobalLocator),
}

impl Representation {
    pub fn file_locator(&self) -> Result<&GlobalLocator> {
        match self {
            Self::Directory => Err(Error::EntryIsDirectory),
            Self::File(locator) => Ok(locator),
        }
    }
}

struct InodeData {
    representation: Representation,
    parent: Inode,
    name: String,
    lookups: u64,
}

pub struct InodeView<'a> {
    inodes: &'a InodeMap,
    data: &'a InodeData,
}

impl<'a> InodeView<'a> {
    pub fn calculate_path(&self) -> Utf8PathBuf {
        self.inodes.calculate_path(self.data)
    }

    pub fn representation(&self) -> &'a Representation {
        &self.data.representation
    }

    pub fn parent(&self) -> Inode {
        self.data.parent
    }
}

#[derive(Eq, PartialEq, Hash)]
struct Key {
    parent: Inode,
    name: String,
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

// Helper to display the full path of an inode. See `InodeMap::path_display` for more info.
struct PathDisplay<'a>(&'a Slab<InodeData>, Inode, Option<&'a str>);

impl fmt::Display for PathDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt_inode_path(f, self.0, self.1)?;

        if let Some(last) = self.2 {
            if self.1 > FUSE_ROOT_ID {
                write!(f, "/")?;
            }

            write!(f, "{}", last)
        } else {
            Ok(())
        }
    }
}

fn fmt_inode_path(f: &mut fmt::Formatter, map: &Slab<InodeData>, inode: Inode) -> fmt::Result {
    let data = &map[inode_to_index(inode)];

    if data.parent > FUSE_ROOT_ID {
        fmt_inode_path(f, map, data.parent)?;
    }

    write!(f, "/{}", data.name)
}
