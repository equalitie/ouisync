use std::{collections::HashSet, io::SeekFrom};

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

use camino::Utf8Path;

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

    /// Looks up an entry by its path. The path must be relative to the repository root.
    /// If the entry exists, returns its `GlobalLocator` and `EntryType`, otherwise returns
    /// `EntryNotFound`.
    pub async fn lookup_type<P: AsRef<Utf8Path>>(&self, path: P) -> Result<EntryType> {
        match decompose_path(path.as_ref()) {
            Some((parent, name)) => {
                let parent = self.open_directory(parent).await?;
                Ok(parent.lookup(name)?.entry_type())
            }
            None => Ok(EntryType::Directory),
        }
    }

    /// Opens a file at the given path (relative to the repository root)
    pub async fn open_file<P: AsRef<Utf8Path>>(&self, path: &P) -> Result<File> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::EntryIsDirectory)?;
        self.open_directory(parent)
            .await?
            .lookup(name)?
            .open_file()
            .await
    }

    /// Opens a directory at the given path (relative to the repository root)
    pub async fn open_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        self.joint_root().await.cd_into_path(path.as_ref()).await
    }

    /// Creates a new file at the given path. Returns the new file and its directory ancestors.
    pub async fn create_file<P: AsRef<Utf8Path>>(
        &self,
        path: &P,
    ) -> Result<(File, Vec<Directory>)> {
        self.local_branch()
            .await
            .ensure_file_exists(path.as_ref())
            .await
    }

    /// Creates a new directory at the given path. Returs a vector of directories corresponding to
    /// the path (starting with the root).
    pub async fn create_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Vec<Directory>> {
        Ok(self
            .local_branch()
            .await
            .ensure_directory_exists(path.as_ref())
            .await?)
    }

    /// Removes (delete) the file at the given path. Returns the parent directory.
    pub async fn remove_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        // TODO: Currently only in local branch.
        self.local_branch().await.remove_file(path).await
    }

    /// Removes the directory at the given path. The directory must be empty. Returns the parent
    /// directory.
    pub async fn remove_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        let (parent, name) = decompose_path(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.open_directory(parent).await?;
        // TODO: Currently only removing directories from the local branch is supported. To
        // implement removing a directory from another branches we need to introduce tombstones.
        parent
            .remove_directory(self.this_replica_id(), name)
            .await?;
        Ok(parent)
    }

    /// Write to a file. If the file is on local branch, it writes to it directly. If it's on a
    /// remote branch, a "copy" of the file is created (blocks are not duplicated, but locators
    /// are) locally and the write then happens to the copy.
    pub async fn write_to_file(
        &self,
        path: &Utf8Path,
        file: &mut File,
        offset: u64,
        buffer: &[u8],
    ) -> Result<()> {
        if &file.global_locator().branch_id != self.this_replica_id() {
            // Perform copy-on-write
            let (parent, name) = decompose_path(&path).ok_or(Error::EntryIsDirectory)?;

            let local_branch = self.local_branch().await;
            let mut local_dirs = local_branch.ensure_directory_exists(parent).await?;

            let local_file_locator = local_dirs
                .last_mut()
                .unwrap() // Always contains root
                .copy_file(name, file.locators(), file.branch())
                .await?;

            let mut new_file = local_branch
                .open_file_by_locator(local_file_locator)
                .await?;

            for dir in local_dirs.iter_mut().rev() {
                dir.flush().await?;
            }

            std::mem::swap(file, &mut new_file);
        }

        file.seek(SeekFrom::Start(offset)).await?;
        file.write(buffer).await
    }

    /// Moves (renames) an entry from the source path to the destination path.
    /// If both source and destination refer to the same entry, this is a no-op.
    /// Returns the parent directories of both `src` and `dst`.
    pub async fn move_entry<S: AsRef<Utf8Path>, D: AsRef<Utf8Path>>(
        &self,
        src: S,
        dst: D,
    ) -> Result<(Directory, MoveDstDirectory)> {
        // TODO: Move entries across branches
        self.local_branch().await.move_entry(src, dst).await
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

    /// Open a file given the GlobalLocator.
    pub async fn open_file_by_locator(&self, locator: &GlobalLocator) -> Result<File> {
        self.branch(&locator.branch_id)
            .await?
            .open_file_by_locator(locator.local)
            .await
    }

    /// Open a directory given the GlobalLocator.
    pub async fn open_directory_by_locator(&self, locator: GlobalLocator) -> Result<Directory> {
        let branch = self.branch(&locator.branch_id).await?;

        if locator.local == Locator::Root && &locator.branch_id == self.this_replica_id() {
            branch.ensure_root_exists().await
        } else {
            branch.open_directory_by_locator(locator.local).await
        }
    }

    /// Returns the local branch
    pub async fn local_branch(&self) -> Branch {
        self.branch(self.this_replica_id()).await.unwrap()
    }

    async fn branch(&self, replica_id: &ReplicaId) -> Result<Branch> {
        self.index
            .branch(replica_id)
            .await
            .map(|branch_data| {
                Branch::new(self.index.pool.clone(), branch_data, self.cryptor.clone())
            })
            .ok_or(Error::EntryNotFound)
    }

    // Opens the root directory across all branches as JointDirectory.
    async fn joint_root(&self) -> JointDirectory {
        let mut root = JointDirectory::new();

        for branch_id in self.branches().await.into_iter() {
            let locator = GlobalLocator {
                branch_id,
                local: Locator::Root,
            };
            if let Ok(dir) = self.open_directory_by_locator(locator).await {
                root.insert(dir);
            } else {
                // Some branch roots may not have been loaded across the network yet. We'll ignore
                // those.
            }
        }

        root
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
