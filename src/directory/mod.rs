mod cache;
mod entry;
pub mod entry_data;
mod entry_type;
mod inner;
mod parent_context;
#[cfg(test)]
mod tests;

pub(crate) use self::{cache::RootDirectoryCache, parent_context::ParentContext};
pub use self::{
    entry::{DirectoryRef, EntryRef, FileRef},
    entry_data::{EntryData, EntryFileData},
    entry_type::EntryType,
};

use self::{
    cache::SubdirectoryCache,
    entry_data::EntryTombstoneData,
    inner::{Content, Inner},
};
use crate::{
    blob::Blob,
    blob_id::BlobId,
    branch::Branch,
    debug_printer::DebugPrinter,
    error::{Error, Result},
    file::File,
    locator::Locator,
    replica_id::ReplicaId,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use std::sync::Arc;
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
    pub async fn flush(&self, version_vector_override: Option<&VersionVector>) -> Result<()> {
        self.write().await.flush(version_vector_override).await
    }

    /// Creates a new file inside this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn create_file(&self, name: String) -> Result<File> {
        self.write().await.create_file(name).await
    }

    /// Creates a new subdirectory of this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn create_directory(&self, name: String) -> Result<Self> {
        let mut inner = self.write().await.inner;

        let author = *self.local_branch.id();

        let blob_id = rand::random();
        let vv = inner
            .entry_version_vector(&name, &author)
            .cloned()
            .unwrap_or_default()
            .incremented(author);

        inner
            .insert_entry(
                name.clone(),
                author,
                EntryData::directory(blob_id, vv),
                None,
            )
            .await?;

        let locator = Locator::Head(blob_id);
        let parent = ParentContext::new(self.inner.clone(), name, author);

        inner
            .open_directories
            .create(self.local_branch.clone(), locator, parent)
            .await
    }

    /// Effectively removes a file from this directory.
    ///
    /// Note: This operation does not simply remove the entry, instead, version vector of the local
    /// entry with the same name is increased to be "happens after" `vv`. If the local version does
    /// not exist, or if it is the one being removed (author == this_replica_id), then a tombstone
    /// is created.
    pub async fn remove_file(
        &self,
        name: &str,
        author: &ReplicaId,
        vv: VersionVector,
    ) -> Result<()> {
        self.write().await.remove_file(name, author, vv, None).await
    }

    /// Removes a subdirectory from this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn remove_directory(&self, name: &str) -> Result<()> {
        let mut inner = self.write().await.inner;

        for entry in lookup(self, &*inner, name)? {
            entry.directory()?.open().await?.remove().await?;
        }

        inner.content.remove_deprecated(name)
        // TODO: Add tombstone
    }

    /// Removes this directory if its empty, otherwise fails.
    pub async fn remove(&self) -> Result<()> {
        let mut inner = self.write().await.inner;

        if !inner.content.entries.is_empty() {
            return Err(Error::DirectoryNotEmpty);
        }

        inner.blob.remove().await
    }

    /// Adds a tombstone to where the entry is being moved from and creates a new entry at the
    /// destination.
    ///
    /// Note on why we're passing the `src_entry` to the function instead of just looking it up
    /// using `src_name` and `src_author`: it's because the version vector inside of the
    /// `src_entry` is expected to be the one the caller of this function is trying to move. It
    /// could, in theory, happen that the source entry has been modified between when the caller
    /// last released the lock to the entry and when we would do the lookup.
    ///
    /// Thus using the "caller provided" version vector, we ensure that we don't accidentally
    /// delete data.
    ///
    /// # Panics
    ///
    /// Panics when `self` (i.e. the source directory) or `dst_dir` are not local.
    pub async fn move_entry(
        &self,
        src_name: &str,
        src_author: &ReplicaId,
        src_entry: EntryData,
        dst_dir: &Directory,
        dst_name: &str,
        dst_vv: VersionVector,
    ) -> Result<()> {
        let (mut src_dir_writer, mut dst_dir_writer) = write_pair(self, dst_dir).await;

        src_dir_writer
            .remove_file(
                src_name,
                src_author,
                src_entry.version_vector().clone(),
                src_entry.blob_id().cloned(),
            )
            .await?;

        // TODO: vv is needlessly created twice here.
        let mut dst_entry = src_entry.clone();
        *dst_entry.version_vector_mut() = dst_vv;

        // TODO: We need to undo the `remove_file` action from above if this next one fails.
        dst_dir_writer
            .as_mut()
            .unwrap_or(&mut src_dir_writer)
            .insert_entry(dst_name.into(), *self.this_replica_id(), dst_entry, None)
            .await
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
            let parent_dir = parent.directory(self.local_branch.clone()).fork().await?;

            if let Ok(entry) = parent_dir
                .read()
                .await
                .lookup_version(parent.entry_name(), self.local_branch.id())
            {
                // TODO: if the local entry exists but is not a directory, we should still create
                // the directory using its owner branch as the author.
                return entry.directory()?.open().await;
            }

            parent_dir
                .create_directory(parent.entry_name().to_owned())
                .await
        } else {
            self.local_branch.open_or_create_root().await
        }
    }

    pub async fn parent(&self) -> Option<Directory> {
        self.read()
            .await
            .inner
            .parent
            .as_ref()
            .map(|parent_ctx| parent_ctx.directory(self.local_branch.clone()))
    }

    /// Inserts a dangling file entry into this directory. It's the responsibility of the caller to
    /// make sure the returned locator eventually points to an actual file.
    /// For internal use only!
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub(crate) async fn insert_file_entry(
        &self,
        name: String,
        author_id: ReplicaId,
        version_vector: VersionVector,
    ) -> Result<BlobId> {
        let mut inner = self.write().await.inner;

        let blob_id = rand::random();
        let entry_data = EntryData::file(blob_id, version_vector);

        inner
            .insert_entry(name, author_id, entry_data, None)
            .await?;

        Ok(blob_id)
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
    pub async fn write(&self) -> Writer<'_> {
        let inner = self.inner.write().await;
        inner.assert_local(self.local_branch.id());
        Writer { outer: self, inner }
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        self.local_branch.id()
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        let inner = self.inner.read().await;

        for (name, versions) in &inner.content.entries {
            print.display(name);
            let print = print.indent();

            for (author, entry_data) in versions {
                print.display(&format!("{:?}: {:?}", author, entry_data));

                if let EntryData::File(file_data) = entry_data {
                    let print = print.indent();

                    let parent_context =
                        ParentContext::new(self.inner.clone(), name.into(), *author);

                    let file = File::open(
                        inner.blob.branch().clone(),
                        self.local_branch.clone(),
                        Locator::Head(file_data.blob_id),
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

impl PartialEq for Directory {
    fn eq(&self, other: &Self) -> bool {
        // We can do this because we have the assurance that if a /foo/bar/ directory is opened
        // more than once, all the opened instances must share the same `.inner`.
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for Directory {}

/// Obtains a write lock for two directories at the same time in a way that prevents deadlock.
///
/// 1. Prevents deadlock when the two directories are actually two instances of the same directory.
///    Returns `(Writer, None)` in that case.
/// 2. Prevents deadlock when two tasks try to lock the same pair of directories concurrently, i.e.
///    one tasks does `write_pair(A, B)` and another does `write_pair(B, A)`. This is achieved by
///    doing the locks in the same order regardless of the order of the arguments. The `Writer`s are
///    always returned in the same order as the arguments however.
///
/// # Panics
///
/// Panics if any of the directories is not in the local branch.
pub async fn write_pair<'a, 'b>(
    a: &'a Directory,
    b: &'b Directory,
) -> (Writer<'a>, Option<Writer<'b>>) {
    if a == b {
        (a.write().await, None)
    } else if Arc::as_ptr(&a.inner) < Arc::as_ptr(&b.inner) {
        let a = a.write().await;
        let b = b.write().await;

        (a, Some(b))
    } else {
        let b = b.write().await;
        let a = a.write().await;

        (a, Some(b))
    }
}

/// View of a `Directory` for performing read and write queries.
pub struct Writer<'a> {
    outer: &'a Directory,
    inner: RwLockWriteGuard<'a, Inner>,
}

impl Writer<'_> {
    pub fn read_operations(&self) -> ReadOperations<'_> {
        ReadOperations {
            outer: self.outer,
            inner: &*self.inner,
        }
    }

    pub async fn create_file(&mut self, name: String) -> Result<File> {
        let author = *self.this_replica_id();

        let blob_id = rand::random();
        let vv = self
            .inner
            .entry_version_vector(&name, &author)
            .cloned()
            .unwrap_or_default()
            .incremented(author);

        self.inner
            .insert_entry(name.clone(), author, EntryData::file(blob_id, vv), None)
            .await?;

        let locator = Locator::Head(blob_id);
        let parent = ParentContext::new(self.outer.inner.clone(), name, author);

        Ok(File::create(
            self.outer.local_branch.clone(),
            locator,
            parent,
        ))
    }

    pub fn lookup_version(&self, name: &'_ str, author: &ReplicaId) -> Result<EntryRef> {
        lookup_version(&*self.inner, self.outer, name, author)
    }

    pub async fn flush(&mut self, version_vector_override: Option<&VersionVector>) -> Result<()> {
        if !self.inner.content.dirty && version_vector_override.is_none() {
            return Ok(());
        }

        let mut tx = self.outer.local_branch.db_pool().begin().await?;

        self.inner.flush(&mut tx).await?;

        self.inner
            .modify_self_entry(tx, *self.outer.local_branch.id(), version_vector_override)
            .await?;

        Ok(())
    }

    pub async fn remove_file(
        &mut self,
        name: &str,
        author: &ReplicaId,
        vv: VersionVector,
        keep: Option<BlobId>,
    ) -> Result<()> {
        let this_replica_id = *self.this_replica_id();

        let new_entry = if author == &this_replica_id {
            EntryData::Tombstone(EntryTombstoneData {
                version_vector: vv.incremented(this_replica_id),
            })
        } else {
            match self.lookup_version(name, &this_replica_id) {
                Ok(local_entry) => {
                    let mut new_entry = local_entry.clone_data();
                    new_entry.version_vector_mut().merge(&vv);
                    new_entry
                }
                Err(Error::EntryNotFound) => EntryData::Tombstone(EntryTombstoneData {
                    version_vector: vv.incremented(this_replica_id),
                }),
                Err(e) => return Err(e),
            }
        };

        self.inner
            .insert_entry(name.into(), this_replica_id, new_entry, keep)
            .await
    }

    pub async fn insert_entry(
        &mut self,
        name: String,
        author: ReplicaId,
        entry: EntryData,
        keep: Option<BlobId>,
    ) -> Result<()> {
        self.inner.insert_entry(name, author, entry, keep).await
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        self.outer.this_replica_id()
    }
}

/// View of a `Directory` for performing read-only queries.
pub struct Reader<'a> {
    outer: &'a Directory,
    inner: RwLockReadGuard<'a, Inner>,
}

impl Reader<'_> {
    pub fn read_operations(&self) -> ReadOperations<'_> {
        ReadOperations {
            outer: self.outer,
            inner: &*self.inner,
        }
    }

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
        lookup_version(&*self.inner, self.outer, name, author)
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

#[derive(Clone)]
pub struct ReadOperations<'a> {
    outer: &'a Directory,
    inner: &'a Inner,
}

impl<'a> ReadOperations<'a> {
    /// Lookup an entry of this directory by name.
    pub fn lookup(
        &self,
        name: &'_ str,
    ) -> Result<impl Iterator<Item = EntryRef<'a>> + ExactSizeIterator + Clone + 'a> {
        lookup(self.outer, self.inner, name)
    }
}

fn lookup_version<'a>(
    inner: &'a Inner,
    outer: &'a Directory,
    name: &str,
    author: &ReplicaId,
) -> Result<EntryRef<'a>> {
    inner
        .content
        .entries
        .get_key_value(name)
        .and_then(|(name, versions)| {
            versions
                .get_key_value(author)
                .map(|(author, data)| EntryRef::new(outer, &*inner, name, data, author))
        })
        .ok_or(Error::EntryNotFound)
}

fn lookup<'a>(
    outer: &'a Directory,
    inner: &'a Inner,
    name: &'_ str,
) -> Result<impl Iterator<Item = EntryRef<'a>> + Clone + ExactSizeIterator> {
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
