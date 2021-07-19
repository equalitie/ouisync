use std::collections::HashSet;

use crate::{
    branch::Branch,
    crypto::Cryptor,
    directory::{Directory, MoveDstDirectory},
    entry::{Entry, EntryType},
    error::{Error, Result},
    file::File,
    global_locator::GlobalLocator,
    index::Index,
    joint_directory::JointDirectory,
    locator::Locator,
    ReplicaId,
};

use camino::{Utf8Component, Utf8Path};

pub struct Repository {
    index: Index,
    cryptor: Cryptor,
}

impl Repository {
    pub fn new(index: Index, cryptor: Cryptor) -> Self {
        Self { index, cryptor }
    }

    pub async fn branches(&self) -> HashSet<ReplicaId> {
        self.index.branch_ids().await
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        self.index.this_replica_id()
    }

    /// Opens the root directory.
    pub async fn joint_root(&self) -> Result<JointDirectory> {
        let mut root = JointDirectory::new();

        for branch_id in self.branches().await.into_iter() {
            let locator = GlobalLocator {
                branch_id,
                local: Locator::Root,
            };
            if let Ok(dir) = self.open_directory_by_locator(locator).await {
                root.insert(dir);
            } else {
            }
        }

        Ok(root)
    }

    /// Looks up an entry by its path. The path must be relative to the repository root.
    /// If the entry exists, returns its `Locator` and `EntryType`, otherwise returns
    /// `EntryNotFound`.
    pub async fn lookup<P: AsRef<Utf8Path>>(&self, path: P) -> Result<(GlobalLocator, EntryType)> {
        self.lookup_by_path(path.as_ref()).await
    }

    /// Opens a file at the given path (relative to the repository root)
    pub async fn open_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let (locator, entry_type) = self.lookup(path).await?;
        entry_type.check_is_file()?;
        self.open_file_by_locator(&locator).await
    }

    /// Opens a directory at the given path (relative to the repository root)
    pub async fn open_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let (locator, entry_type) = self.lookup(path).await?;
        entry_type.check_is_directory()?;
        self.open_directory_by_locator(locator).await
    }

    /// Creates a new file at the given path. Returns both the new file and its parent directory.
    pub async fn create_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<(File, Directory)> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryExists)?;

        let mut parent = self.open_directory(parent).await?;
        let file = parent.create_file(name.to_owned())?;

        Ok((file, parent))
    }

    /// Creates a new directory at the given path. Returns both the new directory and its parent
    /// directory.
    pub async fn create_directory<P: AsRef<Utf8Path>>(
        &self,
        path: P,
    ) -> Result<(Directory, Directory)> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryExists)?;

        let mut parent = self.open_directory(parent).await?;
        let dir = parent.create_directory(name.to_owned())?;

        Ok((dir, parent))
    }

    /// Removes (delete) the file at the given path. Returns the parent directory.
    pub async fn remove_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        let mut parent = self.open_directory(parent).await?;
        parent.remove_file(name).await?;

        Ok(parent)
    }

    /// Removes the directory at the given path. The directory must be empty. Returns the parent
    /// directory.
    pub async fn remove_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        let mut parent = self.open_directory(parent).await?;
        parent.remove_directory(name).await?;

        Ok(parent)
    }

    /// Moves (renames) an entry from the source path to the destination path.
    /// If both source and destination refer to the same entry, this is a no-op.
    /// Returns the parent directories of both `src` and `dst`.
    pub async fn move_entry<S: AsRef<Utf8Path>, D: AsRef<Utf8Path>>(
        &self,
        src: S,
        dst: D,
    ) -> Result<(Directory, MoveDstDirectory)> {
        // `None` here means we are trying to move the root which is not supported.
        let (src_parent, src_name) =
            decompose_path(src.as_ref()).ok_or(Error::OperationNotSupported)?;
        // `None` here means we are trying to move over the root which is a special case of moving
        // over existing directory which is not allowed.
        let (dst_parent, dst_name) = decompose_path(dst.as_ref()).ok_or(Error::EntryIsDirectory)?;

        // TODO: check that dst is not in a subdirectory of src

        let mut src_parent = self.open_directory(src_parent).await?;

        let (dst_parent_locator, dst_parent_type) = self.lookup(dst_parent).await?;
        dst_parent_type.check_is_directory()?;

        let mut dst_parent = if &dst_parent_locator == src_parent.global_locator() {
            MoveDstDirectory::Src
        } else {
            MoveDstDirectory::Other(self.open_directory_by_locator(dst_parent_locator).await?)
        };

        src_parent
            .move_entry(src_name, &mut dst_parent, dst_name)
            .await?;

        Ok((src_parent, dst_parent))
    }

    /// Open an entry (file or directory) at the given locator.
    pub async fn open_entry_by_locator(
        &self,
        locator: GlobalLocator,
        entry_type: EntryType,
    ) -> Result<Entry> {
        match entry_type {
            EntryType::File => Ok(Entry::File(self.open_file_by_locator(&locator).await?)),
            EntryType::Directory => Ok(Entry::Directory(
                self.open_directory_by_locator(locator).await?,
            )),
        }
    }

    pub async fn open_file_by_locator(&self, locator: &GlobalLocator) -> Result<File> {
        let branch = self.branch(&locator.branch_id).await?;
        branch.open_file_by_locator(locator.local).await
    }

    pub async fn open_directory_by_locator(&self, locator: GlobalLocator) -> Result<Directory> {
        let branch = self.branch(&locator.branch_id).await?;

        if locator.local == Locator::Root && &locator.branch_id == self.this_replica_id() {
            branch.ensure_root_exists().await
        }
        else {
            branch.open_directory_by_locator(locator.local).await
        }
    }

    async fn branch(&self, replica_id: &ReplicaId) -> Result<Branch> {
        self.index
            .branch(replica_id)
            .await
            .map(|branch_data|
                 Branch::new(
                     self.index.pool.clone(),
                     branch_data,
                     self.cryptor.clone()
                 ))
            .ok_or(Error::EntryNotFound)
    }

    async fn lookup_by_path(&self, path: &Utf8Path) -> Result<(GlobalLocator, EntryType)> {
        let branch_id = *self.this_replica_id();
        let mut stack = vec![GlobalLocator {
            branch_id,
            local: Locator::Root,
        }];
        let mut last_type = EntryType::Directory;

        for component in path.components() {
            match component {
                Utf8Component::Prefix(_) => return Err(Error::OperationNotSupported),
                Utf8Component::RootDir | Utf8Component::CurDir => (),
                Utf8Component::ParentDir => {
                    if stack.len() > 1 {
                        stack.pop();
                    }

                    last_type = EntryType::Directory;
                }
                Utf8Component::Normal(name) => {
                    last_type.check_is_directory()?;

                    let parent_dir = self
                        .open_directory_by_locator(*stack.last().unwrap())
                        .await?;
                    let next_entry = parent_dir.lookup(name)?;

                    stack.push(GlobalLocator {
                        branch_id,
                        local: next_entry.locator(),
                    });
                    last_type = next_entry.entry_type();
                }
            }
        }

        Ok((stack.pop().unwrap(), last_type))
    }
}

// Decomposes `Path` into parent and filename. Returns `None` if `path` doesn't have parent
// (it's the root).
fn decompose_path(path: &Utf8Path) -> Option<(&Utf8Path, &str)> {
    match (path.parent(), path.file_name()) {
        // It's OK to use unwrap here because all file names are assumed to be UTF-8 (checks are
        // made in VirtualFilesystem).
        (Some(parent), Some(name)) => Some((parent, name)),
        _ => None,
    }
}
