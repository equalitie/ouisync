mod content;
mod entry;
mod entry_data;
mod entry_type;
mod inner;
mod parent_context;
#[cfg(test)]
mod tests;

pub use self::{
    entry::{DirectoryRef, EntryRef, FileRef},
    entry_type::EntryType,
};
pub(crate) use self::{
    entry_data::EntryData, inner::OverwriteStrategy, parent_context::ParentContext,
};

use self::{content::Content, inner::Inner};
use crate::{
    blob::Shared,
    branch::Branch,
    crypto::sign::PublicKey,
    db,
    debug_printer::DebugPrinter,
    error::{Error, Result},
    file::File,
    locator::Locator,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use sqlx::Connection;
use std::mem;

/// Directory access mode
///
/// Note: when a directory is opening in `ReadOnly` mode, it also bypasses the cache. This is
/// because the purpose of the cache is to make sure all instances of the same directory are in
/// sync, but when the directory is read-only, it's not possible to perform any modifications on it
/// that would affect the other instances so the cache is unnecessary. This is also useful when one
/// needs to open the most up to date version of the directory.
#[derive(Clone, Copy)]
pub(crate) enum Mode {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone)]
pub struct Directory {
    mode: Mode,
    inner: Inner,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens the root directory.
    /// For internal use only. Use [`Branch::open_root`] instead.
    pub(crate) async fn open_root(
        conn: &mut db::Connection,
        owner_branch: Branch,
        mode: Mode,
    ) -> Result<Self> {
        Self::open(conn, owner_branch, Locator::ROOT, None, mode).await
    }

    /// Opens the root directory or creates it if it doesn't exist.
    /// For internal use only. Use [`Branch::open_or_create_root`] instead.
    pub(crate) async fn open_or_create_root(
        conn: &mut db::Connection,
        branch: Branch,
    ) -> Result<Self> {
        // TODO: make sure this is atomic
        let locator = Locator::ROOT;

        match Self::open(conn, branch.clone(), locator, None, Mode::ReadWrite).await {
            Ok(dir) => Ok(dir),
            Err(Error::EntryNotFound) => Ok(Self::create(branch, locator, None)),
            Err(error) => Err(error),
        }
    }

    /// Reloads this directory from the db.
    #[cfg(test)]
    pub(crate) async fn refresh(&mut self, conn: &mut db::Connection) -> Result<()> {
        let inner = Inner::open(
            conn,
            self.inner.branch().clone(),
            *self.inner.blob.locator(),
            self.inner.parent.as_ref().cloned(),
        )
        .await?;

        self.inner = inner;

        Ok(())
    }

    /// Lock this directory for reading.
    pub async fn read(&self) -> Reader<'_> {
        Reader {
            outer: self,
            inner: &self.inner,
        }
    }

    /// Creates a new file inside this directory.
    pub async fn create_file(&mut self, conn: &mut db::Connection, name: String) -> Result<File> {
        let mut tx = conn.begin().await?;

        let blob_id = rand::random();
        let data = EntryData::file(blob_id, VersionVector::first(*self.inner.branch().id()));
        let parent = ParentContext::new(
            *self.inner.blob_id(),
            name.clone(),
            self.inner.parent.clone(),
        );
        let shared = self.inner.branch().fetch_blob_shared(blob_id);

        let mut file = File::create(
            self.inner.branch().clone(),
            Locator::head(blob_id),
            parent,
            shared,
        );

        let mut content = self.inner.load(&mut tx).await?;
        content.insert(self.inner.branch(), name, data)?;
        file.save(&mut tx).await?;
        self.inner
            .save(&mut tx, &content, OverwriteStrategy::Remove)
            .await?;
        self.inner.commit(tx, content, VersionVector::new()).await?;

        Ok(file)
    }

    /// Creates a new subdirectory of this directory.
    pub async fn create_directory(
        &mut self,
        conn: &mut db::Connection,
        name: String,
    ) -> Result<Self> {
        let mut tx = conn.begin().await?;

        let blob_id = rand::random();
        let data = EntryData::directory(blob_id, VersionVector::first(*self.inner.branch().id()));
        let parent = ParentContext::new(
            *self.inner.blob_id(),
            name.clone(),
            self.inner.parent.clone(),
        );

        let mut dir = Directory::create(
            self.inner.branch().clone(),
            Locator::head(blob_id),
            Some(parent),
        );

        let mut content = self.inner.load(&mut tx).await?;
        content.insert(self.inner.branch(), name, data)?;
        dir.inner
            .save(&mut tx, &Content::empty(), OverwriteStrategy::Remove)
            .await?;
        self.inner
            .save(&mut tx, &content, OverwriteStrategy::Remove)
            .await?;
        self.inner.commit(tx, content, VersionVector::new()).await?;

        Ok(dir)
    }

    /// Removes a file or subdirectory from this directory. If the entry to be removed is a
    /// directory, it needs to be empty or a `DirectoryNotEmpty` error is returned.
    ///
    /// Note: This operation does not simply remove the entry, instead, version vector of the local
    /// entry with the same name is increased to be "happens after" `vv`. If the local version does
    /// not exist, or if it is the one being removed (branch_id == self.branch_id), then a
    /// tombstone is created.
    pub async fn remove_entry(
        &mut self,
        conn: &mut db::Connection,
        name: &str,
        branch_id: &PublicKey,
        vv: VersionVector,
    ) -> Result<()> {
        let tx = conn.begin().await?;
        self.remove_entry_with_overwrite_strategy(
            tx,
            name,
            branch_id,
            vv,
            OverwriteStrategy::Remove,
        )
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
    /// To move an entry within the same directory, clone `self` and pass it as `dst_dir`.
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
        &mut self,
        conn: &mut db::Connection,
        src_name: &str,
        src_data: EntryData,
        dst_dir: &mut Directory,
        dst_name: &str,
        dst_vv: VersionVector,
    ) -> Result<()> {
        let mut dst_data = src_data;
        let src_vv = mem::replace(dst_data.version_vector_mut(), dst_vv);

        {
            let tx = conn.begin().await?;
            dst_dir
                .insert_entry_with_overwrite_strategy(
                    tx,
                    dst_name.to_owned(),
                    dst_data,
                    OverwriteStrategy::Remove,
                )
                .await?;
        }

        {
            let tx = conn.begin().await?;
            let branch_id = *self.inner.branch().id();
            self.remove_entry_with_overwrite_strategy(
                tx,
                src_name,
                &branch_id,
                src_vv,
                OverwriteStrategy::Keep,
            )
            .await?;
        }

        Ok(())
    }

    // Forks this directory into the local branch.
    // TODO: change this method to modify self instead of returning a new instance.
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
            let dir = parent.directory(conn, inner.branch().clone()).await?;
            let entry_name = parent.entry_name().to_owned();

            Some((dir, entry_name))
        } else {
            None
        };

        // Prevent deadlock
        drop(inner);

        if let Some((parent_dir, entry_name)) = parent {
            let mut parent_dir = parent_dir.fork(conn, local_branch).await?;
            let reader = parent_dir.read().await;

            match reader.lookup(&entry_name) {
                Ok(EntryRef::Directory(entry)) => entry.open(conn).await,
                Ok(EntryRef::File(_)) => {
                    // TODO: return some kind of `Error::Conflict`
                    Err(Error::EntryIsFile)
                }
                Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => {
                    drop(reader);
                    parent_dir.create_directory(conn, entry_name).await
                }
                Err(error) => Err(error),
            }
        } else {
            local_branch.open_or_create_root(conn).await
        }
    }

    /// Updates the version vector of this directory by merging it with `vv`.
    pub(crate) async fn bump(
        &mut self,
        conn: &mut db::Connection,
        vv: VersionVector,
    ) -> Result<()> {
        let tx = conn.begin().await?;
        self.inner.commit(tx, Content::empty(), vv).await
    }

    pub async fn parent(&self, conn: &mut db::Connection) -> Result<Option<Directory>> {
        let read = self.read().await;

        if let Some(parent) = &read.inner.parent {
            Ok(Some(parent.directory(conn, read.branch().clone()).await?))
        } else {
            Ok(None)
        }
    }

    async fn open(
        conn: &mut db::Connection,
        owner_branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
        mode: Mode,
    ) -> Result<Self> {
        Ok(Self {
            mode,
            inner: Inner::open(conn, owner_branch, locator, parent).await?,
        })
    }

    fn create(owner_branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        Directory {
            mode: Mode::ReadWrite,
            inner: Inner::create(owner_branch, locator, parent),
        }
    }

    #[async_recursion]
    pub async fn debug_print(&self, conn: &mut db::Connection, print: DebugPrinter) {
        for (name, entry_data) in self.inner.entries() {
            print.display(&format_args!("{:?}: {:?}", name, entry_data));

            match entry_data {
                EntryData::File(file_data) => {
                    let print = print.indent();

                    let parent_context = ParentContext::new(
                        *self.inner.blob_id(),
                        name.into(),
                        self.inner.parent.clone(),
                    );

                    let file = File::open(
                        conn,
                        self.inner.blob.branch().clone(),
                        Locator::head(file_data.blob_id),
                        parent_context,
                        Shared::uninit(),
                    )
                    .await;

                    match file {
                        Ok(mut file) => {
                            let mut buf = [0; 32];
                            let lenght_result = file.read(conn, &mut buf).await;
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

                    let parent_context = ParentContext::new(
                        *self.inner.blob_id(),
                        name.into(),
                        self.inner.parent.clone(),
                    );

                    let dir = Directory::open(
                        conn,
                        self.inner.blob.branch().clone(),
                        Locator::head(data.blob_id),
                        Some(parent_context),
                        Mode::ReadOnly,
                    )
                    .await;

                    match dir {
                        Ok(dir) => {
                            dir.debug_print(conn, print).await;
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
        self.inner.branch().id()
    }

    pub(crate) async fn version_vector(&self, conn: &mut db::Connection) -> Result<VersionVector> {
        self.read().await.version_vector(conn).await
    }

    async fn remove_entry_with_overwrite_strategy(
        &mut self,
        mut tx: db::Transaction<'_>,
        name: &str,
        branch_id: &PublicKey,
        vv: VersionVector,
        overwrite: OverwriteStrategy,
    ) -> Result<()> {
        let reader = self.read().await;

        // If we are removing a directory, ensure it's empty (recursive removal can still be
        // implemented at the upper layers).
        let old_dir = match reader.lookup(name) {
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

        let new_entry = if branch_id == reader.branch().id() {
            EntryData::tombstone(vv.incremented(*reader.branch().id()))
        } else {
            match reader.lookup(name) {
                Ok(old_entry) => {
                    let mut new_entry = old_entry.clone_data();
                    new_entry.version_vector_mut().merge(&vv);
                    new_entry
                }
                Err(Error::EntryNotFound) => {
                    EntryData::tombstone(vv.incremented(*reader.branch().id()))
                }
                Err(e) => return Err(e),
            }
        };

        drop(reader);

        self.insert_entry_with_overwrite_strategy(tx, name.to_owned(), new_entry, overwrite)
            .await
    }

    /// Atomically inserts the given entry into this directory and commits the change.
    async fn insert_entry_with_overwrite_strategy(
        &mut self,
        mut tx: db::Transaction<'_>,
        name: String,
        entry: EntryData,
        overwrite: OverwriteStrategy,
    ) -> Result<()> {
        let mut content = self.inner.load(&mut tx).await?;
        content.insert(self.inner.branch(), name, entry)?;
        self.inner.save(&mut tx, &content, overwrite).await?;
        self.inner.commit(tx, content, VersionVector::new()).await
    }
}

/// View of a `Directory` for performing read-only queries.
pub struct Reader<'a> {
    outer: &'a Directory,
    inner: &'a Inner,
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
            parent
                .entry_version_vector(conn, self.branch().clone())
                .await
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

fn lookup<'a>(outer: &'a Directory, inner: &'a Inner, name: &'_ str) -> Result<EntryRef<'a>> {
    inner
        .entries()
        .get_key_value(name)
        .map(|(name, data)| EntryRef::new(outer, inner, name, data))
        .ok_or(Error::EntryNotFound)
}
