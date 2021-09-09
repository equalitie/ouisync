mod cache;
mod entry;
mod inner;
mod parent_context;
#[cfg(test)]
mod tests;

pub use self::entry::{DirectoryRef, EntryRef, FileRef};
pub(crate) use self::{cache::RootDirectoryCache, parent_context::ParentContext};

use self::{
    cache::SubdirectoryCache,
    inner::{Content, EntryData, Inner},
};
use crate::{
    blob::Blob,
    blob_id::BlobId,
    branch::Branch,
    db,
    debug_printer::DebugPrinter,
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    locator::Locator,
    replica_id::ReplicaId,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

#[derive(Clone)]
pub struct Directory {
    inner: Arc<RwLock<Inner>>,
    local_branch: Branch,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens the root directory.
    /// For internal use only. Use [`Branch::open_root`] instead.
    pub(crate) async fn open_root(owner_branch: Branch, local_branch: Branch) -> Result<Self> {
        Self::open(owner_branch, local_branch, Locator::Root, None).await
    }

    /// Opens the root directory or creates it if it doesn't exist.
    /// For internal use only. Use [`Branch::open_or_create_root`] instead.
    pub(crate) async fn open_or_create_root(branch: Branch) -> Result<Self> {
        // TODO: make sure this is atomic

        let locator = Locator::Root;

        match Self::open(branch.clone(), branch.clone(), locator, None).await {
            Ok(dir) => Ok(dir),
            Err(Error::EntryNotFound) => Ok(Self::create(branch, locator, None)),
            Err(error) => Err(error),
        }
    }

    /// Lock this directory for reading.
    pub async fn read(&self) -> Reader<'_> {
        Reader {
            outer: self,
            inner: self.inner.read().await,
        }
    }

    /// Flushes this directory ensuring that any pending changes are written to the store and the
    /// version vectors of this and the ancestor directories are properly updated.
    /// Also flushes all ancestor directories.
    ///
    /// The way the version vector is updated depends on the `version_vector_override` parameter.
    /// When it is `None`, the local counter is incremented by one. When it is `Some`, the version
    /// vector is merged with `version_vector_override`. This is useful to support merging
    /// concurrent versions of a directory where the resulting version vector should be the merge
    /// of the version vectors of the concurrent versions.
    pub async fn flush(&mut self, version_vector_override: Option<&VersionVector>) -> Result<()> {
        let mut inner = self.inner.write().await;

        if !inner.content.dirty && version_vector_override.is_none() {
            return Ok(());
        }

        let mut tx = self.local_branch.db_pool().begin().await?;

        inner.flush(&mut tx).await?;

        if let Some(ctx) = inner.parent.as_mut() {
            ctx.directory
                .modify_entry(
                    tx,
                    &ctx.entry_name,
                    &mut ctx.entry_author,
                    version_vector_override,
                )
                .await?
        } else {
            inner
                .blob
                .branch()
                .data()
                .update_root_version_vector(tx, version_vector_override)
                .await?
        }

        Ok(())
    }

    /// Creates a new file inside this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn create_file(&self, name: String) -> Result<File> {
        let mut inner = self.write().await;

        let author = *self.local_branch.id();
        let vv = VersionVector::first(author);
        let blob_id = inner
            .insert_entry(name.clone(), author, EntryType::File, vv)
            .await?;
        let locator = Locator::Head(blob_id);
        let parent = ParentContext {
            directory: self.clone(),
            entry_name: name,
            entry_author: author,
        };

        Ok(File::create(self.local_branch.clone(), locator, parent))
    }

    /// Creates a new subdirectory of this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn create_directory(&self, name: String) -> Result<Self> {
        let mut inner = self.write().await;

        let author = *self.local_branch.id();
        let vv = VersionVector::first(author);
        let blob_id = inner
            .insert_entry(name.clone(), author, EntryType::Directory, vv)
            .await?;
        let locator = Locator::Head(blob_id);
        let parent = ParentContext {
            directory: self.clone(),
            entry_name: name,
            entry_author: author,
        };

        inner
            .open_directories
            .create(self.local_branch.clone(), locator, parent)
            .await
    }

    /// Removes a file from this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn remove_file(&self, name: &str) -> Result<()> {
        let mut inner = self.write().await;

        for entry in lookup(self, &*inner, name)? {
            entry.file()?.open().await?.remove().await?;
        }

        inner.content.remove(name)
        // TODO: Add tombstone
    }

    /// Removes a subdirectory from this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn remove_directory(&self, name: &str) -> Result<()> {
        let mut inner = self.write().await;

        for entry in lookup(self, &*inner, name)? {
            entry.directory()?.open().await?.remove().await?;
        }

        inner.content.remove(name)
        // TODO: Add tombstone
    }

    /// Removes this directory if its empty, otherwise fails.
    pub async fn remove(&self) -> Result<()> {
        let mut inner = self.write().await;

        if !inner.content.entries.is_empty() {
            return Err(Error::DirectoryNotEmpty);
        }

        inner.blob.remove().await
    }

    // Forks this directory into the local branch.
    // TODO: consider changing this to modify self instead of returning the forked dir, to be
    //       consistent with `File::fork`.
    // TODO: consider rewriting this to avoid recursion
    #[async_recursion]
    pub async fn fork(&self) -> Result<Directory> {
        let inner = self.read().await;

        if self.local_branch.id() == inner.branch().id() {
            return Ok(self.clone());
        }

        if let Some(parent) = &inner.inner.parent {
            let parent_dir = parent.directory.fork().await?;

            if let Ok(entry) = parent_dir
                .read()
                .await
                .lookup_version(&parent.entry_name, self.local_branch.id())
            {
                // TODO: if the local entry exists but is not a directory, we should still create
                // the directory using its owner branch as the author.
                return entry.directory()?.open().await;
            }

            parent_dir
                .create_directory(parent.entry_name.to_string())
                .await
        } else {
            self.local_branch.open_or_create_root().await
        }
    }

    /// Inserts a dangling entry into this directory. It's the responsibility of the caller to make
    /// sure the returned locator eventually points to an actual file or directory.
    /// For internal use only!
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub(crate) async fn insert_entry(
        &self,
        name: String,
        author_id: ReplicaId,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<BlobId> {
        let mut inner = self.write().await;
        inner
            .insert_entry(name, author_id, entry_type, version_vector)
            .await
    }

    /// Atomically updates metadata (version vector and author) of the specified entry, then the
    /// metadata of this directory in its parent (and recursively for all ancestors), flushes the
    /// changes to the db and finally commits the db transaction.
    /// If an error occurs anywhere in the process, all intermediate changes are rolled back and all
    /// the affected directories are reverted to their state before calling this function.
    ///
    /// NOTE: This function deliberately deviates from the established API conventions in this
    /// project, namely it modifies the directory, flushes it and commits the db transaction all in
    /// one call. The reason for this is that the correctness of this function depends on the
    /// precise order the various sub-operations are executed and by wrapping it all in one function
    /// we make it harder to misuse. So we decided to sacrifice API purity for corectness.
    #[async_recursion]
    pub(crate) async fn modify_entry<'a>(
        &'a self,
        tx: db::Transaction<'a>,
        name: &'a str,
        author_id: &'a mut ReplicaId,
        version_vector_override: Option<&'a VersionVector>,
    ) -> Result<()> {
        let inner = self.inner.write().await;
        inner.assert_local(self.local_branch.id());

        let mut op = ModifyEntry::new(inner, name, author_id);
        op.apply(*self.local_branch.id(), version_vector_override)?;

        if let Some(parent) = op.inner.parent.as_mut() {
            parent
                .directory
                .modify_entry(tx, &parent.entry_name, &mut parent.entry_author, None)
                .await?;
        } else {
            // This is the last iteration of the recursion. If we reached this it means all the
            // previous operation have suceeded. All that remain is to commit the transaction.
            // If that succeeds, we commit the inidividual `ModifyEntry` operations one by one as
            // we are poping the stack. Becuase those `commit` calls are infallible (it just sets a
            // `bool` flag), it's guaranteed that the whole operation succeeeds after this point.
            tx.commit().await?;
        }

        op.commit();

        Ok(())
    }

    async fn open(
        owner_branch: Branch,
        local_branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
    ) -> Result<Self> {
        let mut blob = Blob::open(owner_branch, locator).await?;
        let buffer = blob.read_to_end().await?;
        let content = bincode::deserialize(&buffer).map_err(Error::MalformedDirectory)?;

        Ok(Self {
            inner: Arc::new(RwLock::new(Inner {
                blob,
                content,
                parent,
                open_directories: SubdirectoryCache::new(),
            })),
            local_branch,
        })
    }

    fn create(owner_branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        let blob = Blob::create(owner_branch.clone(), locator);

        Directory {
            inner: Arc::new(RwLock::new(Inner {
                blob,
                content: Content::new(),
                parent,
                open_directories: SubdirectoryCache::new(),
            })),
            local_branch: owner_branch,
        }
    }

    // Lock this directory for writing.
    //
    // # Panics
    //
    // Panics if not in the local branch.
    async fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        let inner = self.inner.write().await;
        inner.assert_local(self.local_branch.id());
        inner
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        let inner = self.inner.read().await;

        for (name, versions) in &inner.content.entries {
            print.display(name);
            let print = print.indent();

            for (author, entry_data) in versions {
                print.display(&format!(
                    "{:?}: {:?}, blob_id:{:?}, {:?}",
                    author, entry_data.entry_type, entry_data.blob_id, entry_data.version_vector
                ));

                if entry_data.entry_type == EntryType::File {
                    let print = print.indent();

                    let parent_context = ParentContext {
                        directory: self.clone(),
                        entry_name: name.into(),
                        entry_author: *author,
                    };

                    let file = File::open(
                        inner.blob.branch().clone(),
                        self.local_branch.clone(),
                        Locator::Head(entry_data.blob_id),
                        parent_context,
                    )
                    .await;

                    match file {
                        Ok(mut file) => {
                            let mut buf = [0; 32];
                            let lenght_result = file.read(&mut buf).await;
                            match lenght_result {
                                Ok(length) => {
                                    let file_len = file.len().await;
                                    let ellipsis = if file_len > length as u64 { ".." } else { "" };
                                    print.display(&format!(
                                        "Content: {:?}{}",
                                        std::str::from_utf8(&buf[..length]),
                                        ellipsis
                                    ));
                                }
                                Err(e) => {
                                    print.display(&format!("Failed to read {:?}", e));
                                }
                            }
                        }
                        Err(e) => {
                            print.display(&format!("Failed to open {:?}", e));
                        }
                    }
                }
            }
        }
    }
}

/// View of a `Directory` for performing read-only queries.
pub struct Reader<'a> {
    outer: &'a Directory,
    inner: RwLockReadGuard<'a, Inner>,
}

impl Reader<'_> {
    /// Returns iterator over the entries of this directory.
    pub fn entries(&self) -> impl Iterator<Item = EntryRef> + DoubleEndedIterator + Clone {
        self.inner
            .content
            .entries
            .iter()
            .flat_map(move |(name, versions)| {
                versions.iter().map(move |(author, data)| {
                    EntryRef::new(self.outer, &*self.inner, name, data, author)
                })
            })
    }

    /// Lookup an entry of this directory by name.
    pub fn lookup(
        &self,
        name: &'_ str,
    ) -> Result<impl Iterator<Item = EntryRef> + ExactSizeIterator> {
        lookup(self.outer, &*self.inner, name)
    }

    pub fn lookup_version(&self, name: &'_ str, author: &ReplicaId) -> Result<EntryRef> {
        self.inner
            .content
            .entries
            .get_key_value(name)
            .and_then(|(name, versions)| {
                versions.get_key_value(author).map(|(author, data)| {
                    EntryRef::new(self.outer, &*self.inner, name, data, author)
                })
            })
            .ok_or(Error::EntryNotFound)
    }

    /// Length of this directory in bytes. Does not include the content, only the size of directory
    /// itself.
    pub async fn len(&self) -> u64 {
        self.inner.blob.len().await
    }

    /// Locator of this directory
    pub fn locator(&self) -> &Locator {
        self.inner.blob.locator()
    }

    /// Branch of this directory
    pub fn branch(&self) -> &Branch {
        self.inner.blob.branch()
    }

    /// Version vector of this directory.
    pub async fn version_vector(&self) -> VersionVector {
        if let Some(parent) = &self.inner.parent {
            parent.entry_version_vector().await
        } else {
            self.branch().data().root_version_vector().await.clone()
        }
    }

    /// Is this directory in the local branch?
    pub(crate) fn is_local(&self) -> bool {
        self.branch().id() == self.outer.local_branch.id()
    }
}

/// Destination directory of a move operation.
#[allow(clippy::large_enum_variant)]
pub enum MoveDstDirectory {
    /// Same as src
    Src,
    /// Other than src
    Other(Directory),
}

impl MoveDstDirectory {
    pub fn get<'a>(&'a mut self, src: &'a mut Directory) -> &'a mut Directory {
        match self {
            Self::Src => src,
            Self::Other(other) => other,
        }
    }

    pub fn get_other(&mut self) -> Option<&mut Directory> {
        match self {
            Self::Src => None,
            Self::Other(other) => Some(other),
        }
    }
}

fn lookup<'a>(
    outer: &'a Directory,
    inner: &'a Inner,
    name: &'_ str,
) -> Result<impl Iterator<Item = EntryRef<'a>> + ExactSizeIterator> {
    inner
        .content
        .entries
        .get_key_value(name)
        .map(|(name, versions)| {
            versions
                .iter()
                .map(move |(author, data)| EntryRef::new(outer, inner, name, data, author))
        })
        .ok_or(Error::EntryNotFound)
}

/// Helper for the `modify_entry` that allows to undo the operation in case of error.
struct ModifyEntry<'a> {
    inner: RwLockWriteGuard<'a, Inner>,
    name: &'a str,
    author_id: &'a mut ReplicaId,
    orig_author_id: ReplicaId,
    orig_versions: Option<BTreeMap<ReplicaId, EntryData>>,
    orig_dirty: bool,
    committed: bool,
}

impl<'a> ModifyEntry<'a> {
    fn new(
        inner: RwLockWriteGuard<'a, Inner>,
        name: &'a str,
        author_id: &'a mut ReplicaId,
    ) -> Self {
        let orig_author_id = *author_id;
        let orig_versions = inner.content.entries.get(name).cloned();
        let orig_dirty = inner.content.dirty;

        Self {
            inner,
            name,
            author_id,
            orig_author_id,
            orig_versions,
            orig_dirty,
            committed: false,
        }
    }

    // Apply the operation. The operation can still be undone after this by dropping `self`.
    fn apply(
        &mut self,
        local_id: ReplicaId,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        self.inner
            .modify_entry(self.name, self.author_id, local_id, version_vector_override)
    }

    // Commit the operation. After this is called the operation cannot be undone.
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for ModifyEntry<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }

        *self.author_id = self.orig_author_id;
        self.inner.content.dirty = self.orig_dirty;

        if let Some(versions) = self.orig_versions.take() {
            // `unwrap` is OK here because the existence of `versions_backup` implies that the
            // entry for `name` exists because we never remove it during the `modify_entry` call.
            // Also as the `ModifyEntry` struct is holding an exclusive lock (write lock) to the
            // directory internals, it's impossible for someone to remove the entry in the meantime.
            *self.inner.content.entries.get_mut(self.name).unwrap() = versions;
        } else {
            self.inner.content.entries.remove(self.name);
        }
    }
}
