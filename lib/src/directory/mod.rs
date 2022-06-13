mod cache;
mod content;
mod entry;
mod entry_data;
mod entry_type;
mod inner;
mod parent_context;
#[cfg(test)]
mod tests;

pub(crate) use self::{
    cache::RootDirectoryCache, entry_data::EntryData, inner::OverwriteStrategy,
    parent_context::ParentContext,
};
pub use self::{
    entry::{DirectoryRef, EntryRef, FileRef},
    entry_type::EntryType,
};

use self::inner::Inner;
use crate::{
    blob::Shared,
    branch::Branch,
    crypto::sign::PublicKey,
    db,
    debug_printer::DebugPrinter,
    error::{Error, Result},
    file::File,
    locator::Locator,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use sqlx::Connection;
use std::{mem, sync::Arc};

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
    pub(crate) async fn open_root(conn: &mut db::Connection, owner_branch: Branch) -> Result<Self> {
        Self::open(conn, owner_branch, Locator::ROOT, None).await
    }

    /// Opens the root directory or creates it if it doesn't exist.
    /// For internal use only. Use [`Branch::open_or_create_root`] instead.
    pub(crate) async fn open_or_create_root(
        conn: &mut db::Connection,
        branch: Branch,
    ) -> Result<Self> {
        // TODO: make sure this is atomic
        let locator = Locator::ROOT;

        match Self::open(conn, branch.clone(), locator, None).await {
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

    /// Lock this directory for writing.
    async fn write(&self) -> Writer<'_> {
        let inner = self.inner.write().await;
        Writer { outer: self, inner }
    }

    /// Creates a new file inside this directory.
    pub async fn create_file(&self, conn: &mut db::Connection, name: String) -> Result<File> {
        let tx = conn.begin().await?;
        self.write().await.create_file(tx, name).await
    }

    /// Creates a new subdirectory of this directory.
    pub async fn create_directory(&self, conn: &mut db::Connection, name: String) -> Result<Self> {
        let tx = conn.begin().await?;
        self.write().await.create_directory(tx, name).await
    }

    /// Removes a file or subdirectory from this directory. If the entry to be removed is a
    /// directory, it needs to be empty or a `DirectoryNotEmpty` error is returned.
    ///
    /// Note: This operation does not simply remove the entry, instead, version vector of the local
    /// entry with the same name is increased to be "happens after" `vv`. If the local version does
    /// not exist, or if it is the one being removed (branch_id == self.branch_id), then a
    /// tombstone is created.
    pub async fn remove_entry(
        &self,
        conn: &mut db::Connection,
        name: &str,
        branch_id: &PublicKey,
        vv: VersionVector,
    ) -> Result<()> {
        let tx = conn.begin().await?;
        self.write()
            .await
            .remove_entry(tx, name, branch_id, vv, OverwriteStrategy::Remove)
            .await
    }

    /// Adds a tombstone to where the entry is being moved from and creates a new entry at the
    /// destination.
    ///
    /// Note on why we're passing the `src_data` to the function instead of just looking it up
    /// using `src_name`: it's because the version vector inside of the `src_data` is expected to
    /// be the one the caller of this function is trying to move. It could, in theory, happen that
    /// the source entry has been modified between when the caller last released the lock to the
    /// entry and when we would do the lookup.
    ///
    /// Thus using the "caller provided" version vector, we ensure that we don't accidentally
    /// delete data.
    ///
    /// # Cancel safety
    ///
    /// This function is partially cancel safe in the sense that it will never lose data. However,
    /// in the case of cancellation, it can happen that the entry ends up being already inserted
    /// into the destination but not yet removed from the source. This will behave similarly to a
    /// hard link with the important difference that removing the entry from either location  would
    /// leave it dangling in the other. The only way to currently resolve the situation without
    /// removing the data is to copy it somewhere else, then remove both source and destination
    /// entries and then move the data back.
    ///
    /// TODO: Improve cancel-safety either by making the whole operation atomic or by implementing
    /// proper hard links.
    pub(crate) async fn move_entry(
        &self,
        conn: &mut db::Connection,
        src_name: &str,
        src_data: EntryData,
        dst_dir: &Directory,
        dst_name: &str,
        dst_vv: VersionVector,
    ) -> Result<()> {
        let mut dst_data = src_data;
        let src_vv = mem::replace(dst_data.version_vector_mut(), dst_vv);

        {
            let tx = conn.begin().await?;
            let mut dst_writer = dst_dir.write().await;
            dst_writer
                .insert_entry(tx, dst_name.to_owned(), dst_data, OverwriteStrategy::Remove)
                .await?;
        }

        {
            let tx = conn.begin().await?;
            let mut src_writer = self.write().await;
            let branch_id = *src_writer.branch().id();
            src_writer
                .remove_entry(tx, src_name, &branch_id, src_vv, OverwriteStrategy::Keep)
                .await?;
        }

        Ok(())
    }

    // Forks this directory into the local branch.
    #[async_recursion]
    pub(crate) async fn fork(
        &self,
        conn: &mut db::Connection,
        local_branch: &Branch,
    ) -> Result<Directory> {
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

        if let Some((parent_dir, entry_name)) = parent {
            let parent_dir = parent_dir.fork(conn, local_branch).await?;
            let mut writer = parent_dir.write().await;

            match writer.lookup(&entry_name) {
                Ok(EntryRef::Directory(entry)) => entry.open(conn).await,
                Ok(EntryRef::File(_)) => {
                    // TODO: return some kind of `Error::Conflict`
                    Err(Error::EntryIsFile)
                }
                Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => {
                    let tx = conn.begin().await?;
                    writer.create_directory(tx, entry_name).await
                }
                Err(error) => Err(error),
            }
        } else {
            local_branch.open_or_create_root(conn).await
        }
    }

    /// Updates the version vector of this directory by merging it with `vv`.
    pub(crate) async fn bump(&self, conn: &mut db::Connection, vv: VersionVector) -> Result<()> {
        let tx = conn.begin().await?;
        let mut writer = self.write().await;
        writer.inner.commit(tx, vv).await
    }

    pub async fn parent(&self) -> Option<Directory> {
        let read = self.read().await;

        read.inner
            .parent
            .as_ref()
            .map(|parent_ctx| parent_ctx.directory().clone())
    }

    async fn open(
        conn: &mut db::Connection,
        owner_branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
    ) -> Result<Self> {
        Ok(Self {
            branch_id: *owner_branch.id(),
            inner: Arc::new(RwLock::new(
                Inner::open(conn, owner_branch, locator, parent).await?,
            )),
        })
    }

    fn create(owner_branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        Directory {
            branch_id: *owner_branch.id(),
            inner: Arc::new(RwLock::new(Inner::create(owner_branch, locator, parent))),
        }
    }

    #[async_recursion]
    pub async fn debug_print(&self, print: DebugPrinter) {
        let inner = self.inner.read().await;

        for (name, entry_data) in inner.entries() {
            print.display(&format_args!("{:?}: {:?}", name, entry_data));

            match entry_data {
                EntryData::File(file_data) => {
                    let print = print.indent();

                    let parent_context = ParentContext::new(self.clone(), name.into());

                    let mut conn = match inner.branch().db_pool().acquire().await {
                        Ok(conn) => conn,
                        Err(e) => {
                            print.display(&format_args!("Failed to acquire db connection {:?}", e));
                            continue;
                        }
                    };

                    let file = File::open(
                        &mut conn,
                        inner.blob.branch().clone(),
                        Locator::head(file_data.blob_id),
                        parent_context,
                        Shared::uninit().into(),
                    )
                    .await;

                    drop(conn);

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
                EntryData::Directory(data) => {
                    let print = print.indent();

                    let parent_context = ParentContext::new(self.clone(), name.into());

                    let mut conn = match inner.branch().db_pool().acquire().await {
                        Ok(conn) => conn,
                        Err(e) => {
                            print.display(&format_args!("Failed to acquire db connection {:?}", e));
                            continue;
                        }
                    };

                    let dir = inner
                        .open_directories
                        .open(
                            &mut conn,
                            inner.blob.branch().clone(),
                            Locator::head(data.blob_id),
                            parent_context,
                        )
                        .await;

                    drop(conn);

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

    pub fn branch_id(&self) -> &PublicKey {
        &self.branch_id
    }

    pub(crate) async fn version_vector_in_connection(
        &self,
        conn: &mut db::Connection,
    ) -> Result<VersionVector> {
        self.read().await.version_vector(conn).await
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

/// View of a `Directory` for performing read-only queries.
pub struct Reader<'a> {
    outer: &'a Directory,
    inner: RwLockReadGuard<'a, Inner>,
}

impl Reader<'_> {
    /// Returns iterator over the entries of this directory.
    pub fn entries(&self) -> impl Iterator<Item = EntryRef> + DoubleEndedIterator + Clone {
        self.inner
            .entries()
            .iter()
            .map(move |(name, data)| EntryRef::new(self.outer, &*self.inner, name, data))
    }

    /// Lookup an entry of this directory by name.
    pub fn lookup(&self, name: &'_ str) -> Result<EntryRef> {
        lookup(self.outer, &*self.inner, name)
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
    pub async fn version_vector(&self, conn: &mut db::Connection) -> Result<VersionVector> {
        if let Some(parent) = &self.inner.parent {
            Ok(parent.entry_version_vector().await)
        } else {
            self.branch().data().load_version_vector(conn).await
        }
    }

    /// Locator of this directory
    #[cfg(test)]
    pub(crate) fn locator(&self) -> &Locator {
        self.inner.blob.locator()
    }
}

/// View of a `Directory` for performing read and write queries.
struct Writer<'a> {
    outer: &'a Directory,
    inner: RwLockWriteGuard<'a, Inner>,
}

impl Writer<'_> {
    async fn create_file(&mut self, mut tx: db::Transaction<'_>, name: String) -> Result<File> {
        let blob_id = rand::random();
        let shared = Shared::uninit();
        let data = EntryData::file(
            blob_id,
            VersionVector::first(*self.branch().id()),
            shared.downgrade(),
        );
        let parent = ParentContext::new(self.outer.clone(), name.clone());

        let mut file = File::create(
            self.branch().clone(),
            Locator::head(blob_id),
            parent,
            shared,
        );

        self.inner.prepare(&mut tx).await?;
        self.inner.insert(name, data, OverwriteStrategy::Remove)?;
        file.save(&mut tx).await?;
        self.inner.save(&mut tx).await?;
        self.inner.commit(tx, VersionVector::new()).await?;

        Ok(file)
    }

    async fn create_directory(
        &mut self,
        mut tx: db::Transaction<'_>,
        name: String,
    ) -> Result<Directory> {
        let blob_id = rand::random();
        let data = EntryData::directory(blob_id, VersionVector::first(*self.branch().id()));
        let parent = ParentContext::new(self.outer.clone(), name.clone());

        let dir = self
            .inner
            .open_directories
            .create(self.branch(), Locator::head(blob_id), parent)
            .await?;

        self.inner.prepare(&mut tx).await?;
        self.inner.insert(name, data, OverwriteStrategy::Remove)?;
        dir.write().await.inner.save(&mut tx).await?;
        self.inner.save(&mut tx).await?;
        self.inner.commit(tx, VersionVector::new()).await?;

        Ok(dir)
    }

    async fn remove_entry(
        &mut self,
        mut tx: db::Transaction<'_>,
        name: &str,
        branch_id: &PublicKey,
        vv: VersionVector,
        overwrite: OverwriteStrategy,
    ) -> Result<()> {
        // If we are removing a directory, ensure it's empty (recursive removal can still be
        // implemented at the upper layers).
        let old_dir = match self.lookup(name) {
            Ok(EntryRef::Directory(entry)) => Some(entry.open(&mut tx).await?),
            Ok(_) | Err(Error::EntryNotFound) => None,
            Err(error) => return Err(error),
        };

        // Keep the reader (and thus the read lock) around until the end of this function to make
        // sure no new entry is created in the directory after this check in another thread.
        let _old_dir_reader = if let Some(dir) = &old_dir {
            let reader = dir.read().await;

            if matches!(overwrite, OverwriteStrategy::Remove)
                && reader.entries().any(|entry| !entry.is_tombstone())
            {
                return Err(Error::DirectoryNotEmpty);
            }

            Some(reader)
        } else {
            None
        };

        let new_entry = if branch_id == self.branch().id() {
            EntryData::tombstone(vv.incremented(*self.branch().id()))
        } else {
            match self.lookup(name) {
                Ok(old_entry) => {
                    let mut new_entry = old_entry.clone_data();
                    new_entry.version_vector_mut().merge(&vv);
                    new_entry
                }
                Err(Error::EntryNotFound) => {
                    EntryData::tombstone(vv.incremented(*self.branch().id()))
                }
                Err(e) => return Err(e),
            }
        };

        self.insert_entry(tx, name.to_owned(), new_entry, overwrite)
            .await
    }

    fn lookup(&self, name: &'_ str) -> Result<EntryRef> {
        lookup(self.outer, &*self.inner, name)
    }

    fn branch(&self) -> &Branch {
        self.inner.blob.branch()
    }

    /// Atomically inserts the given entry into this directory and commits the change.
    async fn insert_entry(
        &mut self,
        mut tx: db::Transaction<'_>,
        name: String,
        entry: EntryData,
        overwrite: OverwriteStrategy,
    ) -> Result<()> {
        self.inner.prepare(&mut tx).await?;
        self.inner.insert(name, entry, overwrite)?;
        self.inner.save(&mut tx).await?;
        self.inner.commit(tx, VersionVector::new()).await
    }
}

fn lookup<'a>(outer: &'a Directory, inner: &'a Inner, name: &'_ str) -> Result<EntryRef<'a>> {
    inner
        .entries()
        .get_key_value(name)
        .map(|(name, data)| EntryRef::new(outer, inner, name, data))
        .ok_or(Error::EntryNotFound)
}
