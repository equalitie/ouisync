mod content;
mod entry;
mod entry_data;
mod entry_type;
mod parent_context;
#[cfg(test)]
mod tests;

pub use self::{
    entry::{DirectoryRef, EntryRef, FileRef},
    entry_type::EntryType,
};
pub(crate) use self::{
    entry_data::{EntryData, EntryTombstoneData, TombstoneCause},
    parent_context::ParentContext,
};

use self::content::Content;
use crate::{
    blob::{
        lock::{ReadLock, UniqueLock},
        Blob, BlobId,
    },
    branch::Branch,
    crypto::sign::PublicKey,
    debug::DebugPrinter,
    error::{Error, Result},
    file::File,
    protocol::{Locator, RootNode, VersionVectorOp},
    store::{self, LocalWriteTransaction, ReadTransaction},
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use std::{fmt, mem};
use tracing::instrument;

#[derive(Clone)]
pub struct Directory {
    blob: Blob,
    parent: Option<ParentContext>,
    content: Content,
    lock: Option<ReadLock>,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens the root directory.
    /// For internal use only. Use [`Branch::open_root`] instead.
    pub(crate) async fn open_root(
        branch: Branch,
        locking: DirectoryLocking,
        fallback: DirectoryFallback,
    ) -> Result<Self> {
        Self::open(branch, Locator::ROOT, None, locking, fallback).await
    }

    /// Opens the root directory or creates it if it doesn't exist.
    /// For internal use only. Use [`Branch::open_or_create_root`] instead.
    #[instrument(skip_all, fields(branch_id = ?branch.id()))]
    pub(crate) async fn open_or_create_root(branch: Branch) -> Result<Self> {
        let locator = Locator::ROOT;

        let lock = branch.locker().read(*locator.blob_id()).await;
        let mut tx = branch.store().begin_local_write().await?;

        let dir = match Self::open_in(
            Some(lock),
            &mut tx,
            branch.clone(),
            locator,
            None,
            DirectoryFallback::Disabled,
        )
        .await
        {
            Ok(dir) => Some(dir),
            Err(Error::Store(store::Error::BranchNotFound)) => None,
            Err(error) => return Err(error),
        };

        let dir = if let Some(dir) = dir {
            dir
        } else {
            let mut dir = Self::create(branch, locator, None);
            dir.save(&mut tx, &Content::empty()).await?;
            dir.bump(&mut tx, VersionVectorOp::IncrementLocal).await?;
            dir.commit(tx).await?;
            dir
        };

        Ok(dir)
    }

    /// Reloads this directory from the db.
    pub(crate) async fn refresh(&mut self) -> Result<()> {
        let mut tx = self.branch().store().begin_read().await?;
        self.refresh_in(&mut tx).await
    }

    /// Lookup an entry of this directory by name.
    pub fn lookup(&self, name: &'_ str) -> Result<EntryRef> {
        self.content
            .get_key_value(name)
            .map(|(name, data)| EntryRef::new(self, name, data))
            .ok_or(Error::EntryNotFound)
    }

    /// Returns iterator over the entries of this directory.
    pub fn entries(&self) -> impl Iterator<Item = EntryRef> + DoubleEndedIterator + Clone {
        self.content
            .iter()
            .map(move |(name, data)| EntryRef::new(self, name, data))
    }

    /// Creates a new file inside this directory.
    pub async fn create_file(&mut self, name: String) -> Result<File> {
        let mut tx = self.branch().store().begin_local_write().await?;
        self.refresh_in(&mut tx).await?;

        let blob_id = rand::random();
        let version_vector = self
            .content
            .initial_version_vector(&name)
            .incremented(*self.branch().id());
        let data = EntryData::file(blob_id, version_vector);
        let parent = self.create_parent_context(name.clone());

        let mut file = File::create(self.branch().clone(), Locator::head(blob_id), parent);
        let mut content = self.content.clone();

        let _old_lock = content.insert(self.branch(), name, data, None)?;
        file.save(&mut tx).await?;
        self.save(&mut tx, &content).await?;
        self.bump(&mut tx, VersionVectorOp::IncrementLocal).await?;
        self.commit(tx).await?;
        self.finalize(content);

        Ok(file)
    }

    /// Creates a new subdirectory of this directory.
    pub async fn create_directory(&mut self, name: String) -> Result<Self> {
        self.create_directory_with_version_vector_op(name, VersionVectorOp::IncrementLocal)
            .await
    }

    async fn create_directory_with_version_vector_op(
        &mut self,
        name: String,
        op: VersionVectorOp<'_>,
    ) -> Result<Self> {
        let mut tx = self.branch().store().begin_local_write().await?;
        self.refresh_in(&mut tx).await?;

        let blob_id = rand::random();

        let mut version_vector = self.content.initial_version_vector(&name);
        op.apply(self.branch().id(), &mut version_vector);

        let data = EntryData::directory(blob_id, version_vector);
        let parent = self.create_parent_context(name.clone());

        let mut dir =
            Directory::create(self.branch().clone(), Locator::head(blob_id), Some(parent));
        let mut content = self.content.clone();

        let _old_lock = content.insert(self.branch(), name, data, None)?;
        dir.save(&mut tx, &Content::empty()).await?;
        self.save(&mut tx, &content).await?;
        self.bump(&mut tx, op).await?;
        self.commit(tx).await?;
        self.finalize(content);

        Ok(dir)
    }

    /// Opens a subdirectory or creates it if it doesn't exists and merges the given version vector
    /// to it.
    #[instrument(skip(self))]
    pub async fn open_or_create_directory(
        &mut self,
        name: &str,
        merge_vv: &VersionVector,
    ) -> Result<Self> {
        if let Some(dir) = self
            .try_open_directory_and_merge_version_vector(name, merge_vv)
            .await?
        {
            Ok(dir)
        } else {
            match self
                .create_directory_with_version_vector_op(
                    name.to_owned(),
                    VersionVectorOp::Merge(merge_vv),
                )
                .await
            {
                Ok(dir) => Ok(dir),
                Err(Error::EntryExists) => {
                    // The entry have been created in the meantime by some other task. Try to open
                    // it again (this time it must exist).
                    self.try_open_directory_and_merge_version_vector(name, merge_vv)
                        .await
                        .transpose()
                        .expect("entry must exist")
                }
                Err(error) => Err(error),
            }
        }
    }

    async fn try_open_directory_and_merge_version_vector(
        &self,
        name: &str,
        merge_vv: &VersionVector,
    ) -> Result<Option<Self>> {
        let entry = match self.lookup(name) {
            Ok(EntryRef::Directory(entry)) => entry,
            Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => return Ok(None),
            Ok(EntryRef::File(_)) => return Err(Error::EntryIsFile),
            Err(error) => return Err(error),
        };

        let mut dir = entry.open(DirectoryFallback::Disabled).await?;
        dir.merge_version_vector(merge_vv).await?;

        Ok(Some(dir))
    }

    fn create_parent_context(&self, entry_name: String) -> ParentContext {
        ParentContext::new(
            *self.blob_id(),
            self.lock.clone(),
            entry_name,
            self.parent.clone(),
        )
    }

    /// Removes a file or subdirectory from this directory. If the entry to be removed is a
    /// directory, it needs to be empty or a `DirectoryNotEmpty` error is returned.
    ///
    /// Note: This operation does not simply remove the entry, instead, version vector of the local
    /// entry with the same name is increased to be "happens after" `vv`. If the local version does
    /// not exist, or if it is the one being removed (branch_id == self.branch_id), then a
    /// tombstone is created.
    pub(crate) async fn remove_entry(
        &mut self,
        name: &str,
        branch_id: &PublicKey,
        tombstone: EntryTombstoneData,
    ) -> Result<()> {
        let mut tx = self.branch().store().begin_local_write().await?;

        let (content, _old_lock) = match self
            .begin_remove_entry(&mut tx, name, branch_id, tombstone)
            .await
        {
            Ok(content) => content,
            // `EntryExists` in this case means the tombstone already exists which means the entry
            // is already removed.
            Err(Error::EntryExists) => return Ok(()),
            Err(error) => return Err(error),
        };

        self.commit(tx).await?;
        self.finalize(content);

        Ok(())
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
    /// This function is atomic and thus cancel safe. Either the entry is both removed from the src
    /// directory and inserted into the dst directory or, in case of error or cancellation, none of
    /// those operations happen.
    pub(crate) async fn move_entry(
        &mut self,
        src_name: &str,
        src_data: EntryData,
        dst_dir: &mut Directory,
        dst_name: &str,
        dst_vv: VersionVector,
    ) -> Result<()> {
        let mut dst_data = src_data;
        let src_vv = mem::replace(dst_data.version_vector_mut(), dst_vv);

        let mut tx = self.branch().store().begin_local_write().await?;

        let (dst_content, _old_dst_lock) = dst_dir
            .begin_insert_entry(&mut tx, dst_name.to_owned(), dst_data)
            .await?;

        let branch_id = *self.branch().id();
        let (src_content, _old_src_lock) = self
            .begin_remove_entry(
                &mut tx,
                src_name,
                &branch_id,
                EntryTombstoneData::moved(src_vv),
            )
            .await?;

        self.commit(tx).await?;
        self.finalize(src_content);
        dst_dir.finalize(dst_content);

        Ok(())
    }

    // Transfer this directory (but not its content) into the local branch. This effectively
    // creates an empty directory in the loca branch at the same path as `self`. If the local
    // directory already exists, it only updates it's version vector and otherwise does nothing.
    // Note this implicitly forks all the ancestor directories first.
    #[async_recursion]
    pub(crate) async fn fork(&self, local_branch: &Branch) -> Result<Directory> {
        if local_branch.id() == self.branch().id() {
            return Ok(self.clone());
        }

        let parent = if let Some(parent) = &self.parent {
            let dir = parent.open(self.branch().clone()).await?;
            let entry_name = parent.entry_name();

            Some((dir, entry_name))
        } else {
            None
        };

        if let Some((parent_dir, entry_name)) = parent {
            // Because we are transferring only the directory but not its content, we reflect that
            // by setting its version vector to what the version vector of the source directory was
            // at the time it was initially created.
            let vv = VersionVector::first(*self.branch().id());
            let mut parent_dir = parent_dir.fork(local_branch).await?;
            parent_dir.open_or_create_directory(entry_name, &vv).await
        } else {
            local_branch.open_or_create_root().await
        }
    }

    /// Updates the version vector of this directory by merging it with `vv`.
    #[instrument(skip(self), err(Debug))]
    pub(crate) async fn merge_version_vector(&mut self, vv: &VersionVector) -> Result<()> {
        let mut tx = self.branch().store().begin_local_write().await?;
        self.bump(&mut tx, VersionVectorOp::Merge(vv)).await?;
        self.commit(tx).await
    }

    pub async fn parent(&self) -> Result<Option<Directory>> {
        if let Some(parent) = &self.parent {
            Ok(Some(parent.open(self.branch().clone()).await?))
        } else {
            Ok(None)
        }
    }

    async fn open_in(
        lock: Option<ReadLock>,
        tx: &mut ReadTransaction,
        branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
        fallback: DirectoryFallback,
    ) -> Result<Self> {
        let (blob, content) = load(tx, branch, *locator.blob_id(), fallback).await?;

        Ok(Self {
            blob,
            parent,
            content,
            lock,
        })
    }

    async fn open_snapshot(
        tx: &mut ReadTransaction,
        branch: Branch,
        locator: Locator,
        fallback: DirectoryFallback,
    ) -> Result<Content> {
        let (_, content) = load(tx, branch, *locator.blob_id(), fallback).await?;
        Ok(content)
    }

    async fn open(
        branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
        locking: DirectoryLocking,
        fallback: DirectoryFallback,
    ) -> Result<Self> {
        let lock = match locking {
            DirectoryLocking::Enabled => Some(branch.locker().read(*locator.blob_id()).await),
            DirectoryLocking::Disabled => None,
        };

        let mut tx = branch.store().begin_read().await?;
        Self::open_in(lock, &mut tx, branch, locator, parent, fallback).await
    }

    fn create(branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        let lock = branch
            .locker()
            .try_read(*locator.blob_id())
            .ok()
            .expect("blob_id collision");
        let blob = Blob::create(branch, *locator.blob_id());

        Directory {
            blob,
            parent,
            content: Content::empty(),
            lock: Some(lock),
        }
    }

    #[async_recursion]
    pub async fn debug_print(&self, print: DebugPrinter) {
        for (name, entry_data) in &self.content {
            print.display(&format_args!("{:?}: {:?}", name, entry_data));

            match entry_data {
                EntryData::File(file_data) => {
                    let print = print.indent();

                    let parent_context = self.create_parent_context(name.into());
                    let file = File::open(
                        self.blob.branch().clone(),
                        Locator::head(file_data.blob_id),
                        parent_context,
                    )
                    .await;

                    match file {
                        Ok(mut file) => {
                            let mut buf = [0; 32];
                            let lenght_result = file.read(&mut buf).await;
                            match lenght_result {
                                Ok(length) => {
                                    let file_len = file.len();
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

                    let parent_context = self.create_parent_context(name.into());
                    let dir = Directory::open(
                        self.blob.branch().clone(),
                        Locator::head(data.blob_id),
                        Some(parent_context),
                        DirectoryLocking::Disabled,
                        DirectoryFallback::Disabled,
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

    /// Branch of this directory
    pub fn branch(&self) -> &Branch {
        self.blob.branch()
    }

    /// Blob id of this directory
    pub(crate) fn blob_id(&self) -> &BlobId {
        self.blob.id()
    }

    /// Length of this directory in bytes. Does not include the content, only the size of directory
    /// itself.
    pub fn len(&self) -> u64 {
        self.blob.len()
    }

    pub(crate) async fn version_vector(&self) -> Result<VersionVector> {
        if let Some(parent) = &self.parent {
            parent.entry_version_vector(self.branch().clone()).await
        } else {
            self.branch().version_vector().await
        }
    }

    async fn begin_remove_entry(
        &mut self,
        tx: &mut LocalWriteTransaction,
        name: &str,
        branch_id: &PublicKey,
        mut tombstone: EntryTombstoneData,
    ) -> Result<(Content, Option<UniqueLock>)> {
        // If we are removing a directory, ensure it's empty (recursive removal can still be
        // implemented at the upper layers).
        if matches!(tombstone.cause, TombstoneCause::Removed) {
            match self.lookup(name) {
                Ok(EntryRef::Directory(entry)) => {
                    if entry
                        .open_snapshot(tx, DirectoryFallback::Disabled)
                        .await?
                        .iter()
                        .any(|(_, data)| !matches!(data, EntryData::Tombstone(_)))
                    {
                        return Err(Error::DirectoryNotEmpty);
                    }
                }
                Ok(_) | Err(Error::EntryNotFound) => (),
                Err(error) => return Err(error),
            }
        }

        let new_data = match self.lookup(name) {
            Ok(old_entry @ (EntryRef::File(_) | EntryRef::Directory(_))) => {
                if branch_id == self.branch().id() {
                    tombstone.version_vector.increment(*self.branch().id());
                    EntryData::Tombstone(tombstone)
                } else {
                    let mut new_data = old_entry.clone_data();
                    new_data
                        .version_vector_mut()
                        .merge(&tombstone.version_vector);
                    new_data.version_vector_mut().increment(*self.branch().id());
                    new_data
                }
            }
            Ok(EntryRef::Tombstone(_)) => EntryData::Tombstone(tombstone),
            Err(Error::EntryNotFound) => {
                tombstone.version_vector.increment(*self.branch().id());
                EntryData::Tombstone(tombstone)
            }
            Err(e) => return Err(e),
        };

        self.begin_insert_entry(tx, name.to_owned(), new_data).await
    }

    async fn begin_insert_entry(
        &mut self,
        tx: &mut LocalWriteTransaction,
        name: String,
        data: EntryData,
    ) -> Result<(Content, Option<UniqueLock>)> {
        self.refresh_in(tx).await?;

        let mut content = self.content.clone();
        let old_lock = content.insert(self.branch(), name, data, None)?;
        self.save(tx, &content).await?;
        self.bump(tx, VersionVectorOp::IncrementLocal).await?;

        Ok((content, old_lock))
    }

    async fn refresh_in(&mut self, tx: &mut ReadTransaction) -> Result<()> {
        if self.blob.is_dirty() {
            Ok(())
        } else {
            let (blob, content) = load(
                tx,
                self.branch().clone(),
                *self.blob_id(),
                DirectoryFallback::Disabled,
            )
            .await?;
            self.blob = blob;
            self.content = content;

            Ok(())
        }
    }

    async fn save(&mut self, tx: &mut LocalWriteTransaction, content: &Content) -> Result<()> {
        // Save the directory content into the store
        let buffer = content.serialize();
        self.blob.truncate(0)?;
        self.blob.write_all(tx, &buffer).await?;
        self.blob.flush(tx).await?;

        Ok(())
    }

    /// Atomically commits the transaction and sends notification event.
    async fn commit(&mut self, tx: LocalWriteTransaction) -> Result<()> {
        let event_tx = self.branch().notify();
        tx.commit_and_then(move || event_tx.send()).await?;
        Ok(())
    }

    /// Updates the version vectors of this directory and all its ancestors.
    #[async_recursion]
    async fn bump<'a: 'async_recursion>(
        &mut self,
        tx: &mut LocalWriteTransaction,
        op: VersionVectorOp<'a>,
    ) -> Result<()> {
        // Update the version vector of this directory and all it's ancestors
        if let Some(parent) = self.parent.as_mut() {
            parent.bump(tx, self.blob.branch().clone(), op).await
        } else {
            let write_keys = self
                .branch()
                .keys()
                .write()
                .ok_or(Error::PermissionDenied)?;

            tx.bump(self.branch().id(), op, write_keys).await?;

            Ok(())
        }
    }

    /// Finalize pending modifications. Call this only after the db transaction has been committed.
    fn finalize(&mut self, content: Content) {
        self.content = content;
    }
}

impl fmt::Debug for Directory {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Directory")
            .field(
                "name",
                &self
                    .parent
                    .as_ref()
                    .map(|parent| parent.entry_name())
                    .unwrap_or("/"),
            )
            .field("branch", self.branch().id())
            .field("blob_id", self.blob_id())
            .finish()
    }
}

/// Enable/disable fallback to previous snapshots in case of missing blocks.
#[derive(Clone, Copy)]
pub(crate) enum DirectoryFallback {
    Enabled,
    Disabled,
}

/// Enable/disable acquiring read lock before creating/opening the directory.
#[derive(Clone, Copy)]
pub(crate) enum DirectoryLocking {
    Enabled,
    Disabled,
}

// Load directory content. On missing block, fallback to previous snapshot (if any).
async fn load(
    tx: &mut ReadTransaction,
    branch: Branch,
    blob_id: BlobId,
    fallback: DirectoryFallback,
) -> Result<(Blob, Content)> {
    let mut root_node = tx.load_root_node(branch.id()).await?;

    loop {
        let error = match load_at(tx, &root_node, branch.clone(), blob_id).await {
            Ok((blob, content)) => return Ok((blob, content)),
            Err(error @ Error::Store(store::Error::BlockNotFound)) => error,
            Err(error) => return Err(error),
        };

        match fallback {
            DirectoryFallback::Enabled => (),
            DirectoryFallback::Disabled => return Err(error),
        }

        if let Some(prev) = tx.load_prev_root_node(&root_node).await? {
            root_node = prev;
        } else {
            return Err(error);
        }
    }
}

async fn load_at(
    tx: &mut ReadTransaction,
    root_node: &RootNode,
    branch: Branch,
    blob_id: BlobId,
) -> Result<(Blob, Content)> {
    let mut blob = Blob::open_at(tx, root_node, branch, blob_id).await?;
    let buffer = blob.read_to_end_at(tx, root_node).await?;
    let content = Content::deserialize(&buffer)?;

    Ok((blob, content))
}
