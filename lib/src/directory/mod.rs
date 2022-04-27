mod cache;
mod entry;
mod entry_data;
mod entry_type;
mod inner;
mod parent_context;
#[cfg(test)]
mod tests;

pub(crate) use self::{
    cache::RootDirectoryCache, entry_data::EntryData, parent_context::ParentContext,
};
pub use self::{
    entry::{DirectoryRef, EntryRef, FileRef},
    entry_type::EntryType,
};

use self::{cache::SubdirectoryCache, entry_data::EntryTombstoneData, inner::Inner};
use crate::{
    blob::{Blob, Shared},
    blob_id::BlobId,
    branch::Branch,
    crypto::sign::PublicKey,
    debug_printer::DebugPrinter,
    error::{Error, Result},
    file::File,
    locator::Locator,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use sqlx::Connection;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct Directory {
    // `branch_id` is equivalent `inner.read().await.branch().id()`, but access to it doesn't
    // require locking.
    branch_id: PublicKey,
    inner: Arc<RwLock<Inner>>,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens the root directory.
    /// For internal use only. Use [`Branch::open_root`] instead.
    pub(crate) async fn open_root(owner_branch: Branch) -> Result<Self> {
        Self::open(owner_branch, Locator::ROOT, None).await
    }

    /// Opens the root directory or creates it if it doesn't exist.
    /// For internal use only. Use [`Branch::open_or_create_root`] instead.
    pub(crate) async fn open_or_create_root(branch: Branch) -> Result<Self> {
        // TODO: make sure this is atomic

        let locator = Locator::ROOT;

        match Self::open(branch.clone(), locator, None).await {
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
    pub async fn flush(&self) -> Result<()> {
        self.write().await.flush().await
    }

    /// Creates a new file inside this directory.
    pub async fn create_file(&self, name: String) -> Result<File> {
        self.write().await.create_file(name).await
    }

    /// Creates a new subdirectory of this directory.
    pub async fn create_directory(&self, name: String) -> Result<Self> {
        self.write().await.create_directory(name).await
    }

    /// Removes a file or subdirectory from this directory. If the entry to be removed is a
    /// directory, it needs to be empty or a `DirectoryNotEmpty` error is returned.
    ///
    /// Note: This operation does not simply remove the entry, instead, version vector of the local
    /// entry with the same name is increased to be "happens after" `vv`. If the local version does
    /// not exist, or if it is the one being removed (author == this_writer_id), then a tombstone
    /// is created.
    pub async fn remove_entry(
        &self,
        name: &str,
        author: &PublicKey,
        vv: VersionVector,
    ) -> Result<()> {
        self.write()
            .await
            .remove_entry(name, author, vv, None)
            .await
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
    pub(crate) async fn move_entry(
        &self,
        src_name: &str,
        src_author: &PublicKey,
        src_entry: EntryData,
        dst_dir: &Directory,
        dst_name: &str,
        dst_vv: VersionVector,
    ) -> Result<()> {
        let (mut src_dir_writer, mut dst_dir_writer) = write_pair(self, dst_dir).await;

        src_dir_writer
            .remove_entry(
                src_name,
                src_author,
                src_entry.version_vector().clone(),
                src_entry.blob_id().cloned(),
            )
            .await?;

        // TODO: vv is needlessly created twice here.
        let mut dst_entry = src_entry;
        *dst_entry.version_vector_mut() = dst_vv;

        let dst_author = dst_dir.branch_id;

        // TODO: We need to undo the `remove_file` action from above if this next one fails.
        dst_dir_writer
            .as_mut()
            .unwrap_or(&mut src_dir_writer)
            .insert_entry(dst_name.into(), dst_author, dst_entry)
            .await
    }

    // Forks this directory into the local branch.
    // TODO: consider changing this to modify self instead of returning the forked dir, to be
    //       consistent with `File::fork`.
    // TODO: consider rewriting this to avoid recursion
    #[async_recursion]
    pub async fn fork(&self, local_branch: &Branch) -> Result<Directory> {
        let inner = self.read().await;

        if local_branch.id() == inner.branch().id() {
            return Ok(self.clone());
        }

        let parent = if let Some(parent) = &inner.inner.parent {
            let dir = parent.directory().clone();
            let entry_name = parent.entry_name().to_owned();

            Some((dir, entry_name))
        } else {
            None
        };

        // Prevent deadlock
        drop(inner);

        if let Some((parent_dir, parent_entry_name)) = parent {
            let parent_dir = parent_dir.fork(local_branch).await?;

            match parent_dir
                .read()
                .await
                .lookup_version(&parent_entry_name, local_branch.id())
            {
                Ok(EntryRef::Directory(entry)) => return entry.open().await,
                Ok(EntryRef::File(_)) => {
                    // FIXME: create the directory alongside the file somehow.
                    return Err(Error::EntryIsFile);
                }
                Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => (),
                Err(error) => return Err(error),
            }

            parent_dir.create_directory(parent_entry_name).await
        } else {
            local_branch.open_or_create_root().await
        }
    }

    pub async fn parent(&self) -> Option<Directory> {
        let read = self.read().await;

        read.inner
            .parent
            .as_ref()
            .map(|parent_ctx| parent_ctx.directory().clone())
    }

    async fn open(
        owner_branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
    ) -> Result<Self> {
        let branch_id = *owner_branch.id();
        let mut blob = Blob::open(owner_branch, locator, Shared::uninit().into()).await?;
        let buffer = blob.read_to_end().await?;
        let entries = bincode::deserialize(&buffer).map_err(Error::MalformedDirectory)?;

        Ok(Self {
            branch_id,
            inner: Arc::new(RwLock::new(Inner {
                blob,
                entries,
                version_vector_increment: VersionVector::new(),
                parent,
                open_directories: SubdirectoryCache::new(),
            })),
        })
    }

    fn create(owner_branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        let branch_id = *owner_branch.id();
        let blob = Blob::create(owner_branch, locator, Shared::uninit());

        Directory {
            branch_id,
            inner: Arc::new(RwLock::new(Inner {
                blob,
                entries: BTreeMap::new(),
                version_vector_increment: VersionVector::first(branch_id),
                parent,
                open_directories: SubdirectoryCache::new(),
            })),
        }
    }

    pub async fn open_file(&self, name: &str, author: &PublicKey) -> Result<File> {
        // IMPORTANT: make sure the parent directory is unlocked before `await`-ing the `open`
        // future, to avoid deadlocks.
        let open = {
            self.read()
                .await
                .lookup_version(name, author)?
                .file()?
                .open()
        };

        open.await
    }

    pub async fn open_directory(&self, name: &str, author: &PublicKey) -> Result<Directory> {
        self.read()
            .await
            .lookup_version(name, author)?
            .directory()?
            .open()
            .await
    }

    // Lock this directory for writing.
    pub(crate) async fn write(&self) -> Writer<'_> {
        let inner = self.inner.write().await;
        Writer { outer: self, inner }
    }

    #[async_recursion]
    pub async fn debug_print(&self, print: DebugPrinter) {
        let inner = self.inner.read().await;

        for (name, versions) in &inner.entries {
            print.display(name);
            let print = print.indent();

            for (author, entry_data) in versions {
                print.display(&format!("{:?}: {:?}", author, entry_data));

                match entry_data {
                    EntryData::File(file_data) => {
                        let print = print.indent();

                        let parent_context = ParentContext::new(self.clone(), name.into(), *author);

                        let file = File::open(
                            inner.blob.branch().clone(),
                            Locator::head(file_data.blob_id),
                            parent_context,
                            Shared::uninit().into(),
                        )
                        .await;

                        match file {
                            Ok(mut file) => {
                                let mut buf = [0; 32];
                                let lenght_result = file.read(&mut buf).await;
                                match lenght_result {
                                    Ok(length) => {
                                        let file_len = file.len().await;
                                        let ellipsis =
                                            if file_len > length as u64 { ".." } else { "" };
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
                    EntryData::Directory(data) => {
                        let print = print.indent();

                        let parent_context = ParentContext::new(self.clone(), name.into(), *author);

                        let dir = inner
                            .open_directories
                            .open(
                                inner.blob.branch().clone(),
                                Locator::head(data.blob_id),
                                parent_context,
                            )
                            .await;

                        match dir {
                            Ok(dir) => {
                                dir.debug_print(print).await;
                            }
                            Err(e) => {
                                print.display(&format!("Failed to open {:?}", e));
                            }
                        }
                    }
                    EntryData::Tombstone(_) => {}
                }
            }
        }
    }

    pub fn branch_id(&self) -> &PublicKey {
        &self.branch_id
    }

    #[cfg(test)]
    pub(crate) async fn version_vector(&self) -> VersionVector {
        self.read().await.version_vector().await
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
pub(crate) async fn write_pair<'a, 'b>(
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
pub(crate) struct Writer<'a> {
    outer: &'a Directory,
    inner: RwLockWriteGuard<'a, Inner>,
}

impl Writer<'_> {
    pub async fn create_file(&mut self, name: String) -> Result<File> {
        let author = *self.branch().id();
        let blob_id = rand::random();
        let shared = Shared::uninit();
        let data = EntryData::file(blob_id, VersionVector::new(), shared.downgrade());
        let parent = ParentContext::new(self.outer.clone(), name.clone(), author);

        self.inner.insert_entry(name, author, data, None).await?;

        Ok(File::create(
            self.branch().clone(),
            Locator::head(blob_id),
            parent,
            shared,
        ))
    }

    pub async fn create_directory(&mut self, name: String) -> Result<Directory> {
        let author = *self.branch().id();
        let blob_id = rand::random();
        let data = EntryData::directory(blob_id, VersionVector::new());
        let parent = ParentContext::new(self.outer.clone(), name.clone(), author);

        self.inner.insert_entry(name, author, data, None).await?;

        self.inner
            .open_directories
            .create(self.branch(), Locator::head(blob_id), parent)
            .await
    }

    pub fn lookup_version(&self, name: &'_ str, author: &PublicKey) -> Result<EntryRef> {
        lookup_version(&*self.inner, self.outer, name, author)
    }

    pub async fn flush(&mut self) -> Result<()> {
        if self.inner.version_vector_increment.is_empty() {
            return Ok(());
        }

        let mut conn = self.inner.blob.db_pool().acquire().await?;
        let mut tx = conn.begin().await?;
        self.inner.flush(&mut tx).await?;
        self.inner.modify_self_entry(tx).await?;

        Ok(())
    }

    pub async fn remove_entry(
        &mut self,
        name: &str,
        author: &PublicKey,
        vv: VersionVector,
        // `keep` is set when we're moving the entry and only want to remove it from the entries
        // listed in `self` (as opposed to also remove its data).
        keep: Option<BlobId>,
    ) -> Result<()> {
        // If we are removing a directory, ensure it's empty (recursive removal can still be
        // implemented at the upper layers).
        let old_dir = match self.lookup_version(name, author) {
            Ok(EntryRef::Directory(entry)) => Some(entry.open().await?),
            Ok(_) | Err(Error::EntryNotFound) => None,
            Err(error) => return Err(error),
        };

        // Keep the reader (and thus the read lock) around until the end of this function to make
        // sure no new entry is created in the directory after this check in another thread.
        let _old_dir_reader = if let Some(dir) = &old_dir {
            let reader = dir.read().await;

            if keep.is_none() && reader.entries().any(|entry| !entry.is_tombstone()) {
                return Err(Error::DirectoryNotEmpty);
            }

            Some(reader)
        } else {
            None
        };

        let this_writer_id = *self.branch().id();

        let new_entry = if author == &this_writer_id {
            EntryData::Tombstone(EntryTombstoneData {
                version_vector: vv.incremented(this_writer_id),
            })
        } else {
            match self.lookup_version(name, &this_writer_id) {
                Ok(local_entry) => {
                    let mut new_entry = local_entry.clone_data();
                    new_entry.version_vector_mut().merge(&vv);
                    new_entry
                }
                Err(Error::EntryNotFound) => EntryData::Tombstone(EntryTombstoneData {
                    version_vector: vv.incremented(this_writer_id),
                }),
                Err(e) => return Err(e),
            }
        };

        self.inner
            .insert_entry(name.into(), this_writer_id, new_entry, keep)
            .await
    }

    pub async fn insert_entry(
        &mut self,
        name: String,
        author: PublicKey,
        entry: EntryData,
    ) -> Result<()> {
        self.inner.insert_entry(name, author, entry, None).await
    }

    pub fn branch(&self) -> &Branch {
        self.inner.blob.branch()
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
        self.inner.entries.iter().flat_map(move |(name, versions)| {
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

    pub fn lookup_version(&self, name: &'_ str, author: &PublicKey) -> Result<EntryRef> {
        lookup_version(&*self.inner, self.outer, name, author)
    }

    /// Length of this directory in bytes. Does not include the content, only the size of directory
    /// itself.
    pub async fn len(&self) -> u64 {
        self.inner.blob.len().await
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
            self.branch()
                .data()
                .root()
                .await
                .proof
                .version_vector
                .clone()
        }
    }

    /// Locator of this directory
    #[cfg(test)]
    pub(crate) fn locator(&self) -> &Locator {
        self.inner.blob.locator()
    }
}

fn lookup_version<'a>(
    inner: &'a Inner,
    outer: &'a Directory,
    name: &str,
    author: &PublicKey,
) -> Result<EntryRef<'a>> {
    inner
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
        .entries
        .get_key_value(name)
        .map(|(name, versions)| {
            versions
                .iter()
                .map(move |(author, data)| EntryRef::new(outer, inner, name, data, author))
        })
        .ok_or(Error::EntryNotFound)
}
