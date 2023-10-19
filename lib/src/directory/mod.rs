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
    blob::{lock::ReadLock, Blob, BlobId},
    branch::Branch,
    crypto::sign::PublicKey,
    debug::DebugPrinter,
    error::{Error, Result},
    file::File,
    protocol::{Bump, Locator, RootNode, RootNodeFilter},
    store::{self, Changeset, ReadTransaction, WriteTransaction},
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
    pub(crate) async fn open_root(
        branch: Branch,
        locking: DirectoryLocking,
        fallback: DirectoryFallback,
    ) -> Result<Self> {
        Self::open(branch, Locator::ROOT, None, locking, fallback).await
    }

    /// Opens the root directory or creates it if it doesn't exists.
    ///
    /// See [`Self::create_directory`] for info about the `merge` parameter.
    #[instrument(skip(branch), fields(branch_id = ?branch.id()))]
    pub(crate) async fn open_or_create_root(branch: Branch, merge: VersionVector) -> Result<Self> {
        let locator = Locator::ROOT;

        let lock = branch.locker().read(*locator.blob_id()).await;
        let mut tx = branch.store().begin_write().await?;
        let mut changeset = Changeset::new();

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

        let dir = if let Some(mut dir) = dir {
            if !merge.is_empty() {
                let bump = Bump::Merge(merge);
                dir.bump(&mut tx, &mut changeset, bump).await?;
                dir.commit(tx, changeset).await?;
            }

            dir
        } else {
            let bump = if merge.is_empty() {
                Bump::increment(*branch.id())
            } else {
                Bump::Merge(merge)
            };

            let mut dir = Self::create(branch, locator, None);
            dir.save(&mut tx, &mut changeset, &Content::empty()).await?;
            dir.bump(&mut tx, &mut changeset, bump).await?;
            dir.commit(tx, changeset).await?;
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
        let mut tx = self.branch().store().begin_write().await?;
        let mut changeset = Changeset::new();

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

        let diff = content.insert(name, data)?;

        file.save(&mut tx, &mut changeset).await?;
        self.save(&mut tx, &mut changeset, &content).await?;
        self.bump(&mut tx, &mut changeset, Bump::Add(diff)).await?;
        self.commit(tx, changeset).await?;
        self.finalize(content);

        Ok(file)
    }

    /// Creates a new subdirectory of this directory.
    ///
    /// The version vector of the created subdirectory depends on the `merge` parameter:
    ///
    /// - if it is empty, it's `old_vv` with the local version incremented,
    /// - if it is non-empty, it's the `old_vv` merged with `merge`,
    ///
    /// where `old_vv` is the version vector of the existing entry or `VersionVector::new()` if not
    /// such exists yet.
    #[instrument(level = "trace", skip(self))]
    pub async fn create_directory(&mut self, name: String, merge: &VersionVector) -> Result<Self> {
        let mut tx = self.branch().store().begin_write().await?;
        let mut changeset = Changeset::new();

        self.refresh_in(&mut tx).await?;

        let blob_id = rand::random();

        let mut version_vector = self.content.initial_version_vector(&name);

        if merge.is_empty() {
            version_vector.increment(*self.branch().id())
        } else {
            version_vector.merge(merge)
        }

        let data = EntryData::directory(blob_id, version_vector);
        let parent = self.create_parent_context(name.clone());

        let mut dir =
            Directory::create(self.branch().clone(), Locator::head(blob_id), Some(parent));
        let mut content = self.content.clone();

        let diff = content.insert(name, data)?;

        dir.save(&mut tx, &mut changeset, &Content::empty()).await?;
        self.save(&mut tx, &mut changeset, &content).await?;
        self.bump(&mut tx, &mut changeset, Bump::Add(diff)).await?;
        self.commit(tx, changeset).await?;
        self.finalize(content);

        Ok(dir)
    }

    /// Opens a subdirectory or creates it if it doesn't exists and merges the given version vector
    /// to it.
    #[instrument(skip(self))]
    pub async fn open_or_create_directory(
        &mut self,
        name: &str,
        merge: VersionVector,
    ) -> Result<Self> {
        let mut dir = if let Some(dir) = self.try_open_directory(name).await? {
            dir
        } else {
            match self.create_directory(name.to_owned(), &merge).await {
                Ok(dir) => return Ok(dir),
                Err(Error::EntryExists) => {
                    // The entry have been created in the meantime by some other task. Try to open
                    // it again (this time it must exist).
                    self.try_open_directory(name)
                        .await?
                        .expect("entry must exist")
                }
                Err(error) => return Err(error),
            }
        };

        if !merge.is_empty() {
            let mut tx = self.branch().store().begin_write().await?;
            let mut changeset = Changeset::new();
            let bump = Bump::Merge(merge);
            dir.bump(&mut tx, &mut changeset, bump).await?;
            dir.commit(tx, changeset).await?;
        }

        Ok(dir)
    }

    async fn try_open_directory(&self, name: &str) -> Result<Option<Self>> {
        let entry = match self.lookup(name) {
            Ok(EntryRef::Directory(entry)) => entry,
            Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => return Ok(None),
            Ok(EntryRef::File(_)) => return Err(Error::EntryIsFile),
            Err(error) => return Err(error),
        };

        let dir = entry.open(DirectoryFallback::Disabled).await?;

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
    /// `branch_id` and `version_vector` specify the entry to be removed. This makes it possible to
    /// remove entries from remote branches as well.
    ///
    /// In most cases the removal is implemented by inserting a "tombstone" in place of the entry.
    /// One exception is when removing a remote entry which also has a concurrent local version. In
    /// that case the version vector of the local entry is bumped to be happens-after the one to be
    /// removed. This effectively removes the remote entry while keeping the local one.
    #[instrument(skip(self))]
    pub(crate) async fn remove_entry(
        &mut self,
        name: &str,
        branch_id: &PublicKey,
        version_vector: VersionVector,
    ) -> Result<()> {
        let mut tx = self.branch().store().begin_write().await?;
        let mut changeset = Changeset::new();

        // If we are removing a directory, ensure it's empty (recursive removal can still be
        // implemented at the upper layers).
        self.check_directory_empty(&mut tx, name).await?;

        let content = self
            .begin_remove_entry(
                &mut tx,
                &mut changeset,
                name,
                branch_id,
                version_vector,
                TombstoneCause::Removed,
            )
            .await?;

        self.commit(tx, changeset).await?;
        self.finalize(content);

        Ok(())
    }

    /// Creates a tombstone for entry with the given name. If the entry exists, this effectively
    /// removes it. If it doesn't exist, it still creates the tombstone. This method is meant to be
    /// used for merging removed entries from other branches. For removing entries locally, use
    /// [`Self::remove_entry`] instead.
    #[instrument()]
    pub(crate) async fn create_tombstone(
        &mut self,
        name: &str,
        tombstone: EntryTombstoneData,
    ) -> Result<()> {
        let mut tx = self.branch().store().begin_write().await?;
        let mut changeset = Changeset::new();

        match self.lookup(name) {
            Ok(EntryRef::File(_) | EntryRef::Directory(_)) | Err(Error::EntryNotFound) => (),
            Ok(EntryRef::Tombstone(old_entry)) => {
                // Attempt to replace a tombstone with another tombstone whose version vector is
                // the same or lower is a no-op.
                if &tombstone.version_vector <= old_entry.version_vector() {
                    return Ok(());
                }
            }
            Err(e) => return Err(e),
        }

        let content = self
            .begin_insert_entry(
                &mut tx,
                &mut changeset,
                name.to_owned(),
                EntryData::Tombstone(tombstone),
            )
            .await?;

        self.commit(tx, changeset).await?;
        self.finalize(content);

        Ok(())
    }

    /// Moves an entry at `src_name` from this directory to the `dst_dir` directory at `dst_name`.
    ///
    /// It adds a tombstone to where the entry is being moved from and creates a new entry at the
    /// destination.
    ///
    /// Note on why we're passing the `src_data` to the function instead of just looking it up
    /// using `src_name`: it's because the version vector inside of the `src_data` is expected to
    /// be the one the caller of this function is trying to move. It could, in theory, happen that
    /// the source entry has been modified between when the caller obtained the entry and when we
    /// would do the lookup.
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

        let mut tx = self.branch().store().begin_write().await?;

        let mut changeset = Changeset::new();
        let dst_content = dst_dir
            .begin_insert_entry(&mut tx, &mut changeset, dst_name.to_owned(), dst_data)
            .await?;

        // TODO: Handle the case when `self` == `dst_dir` separately (call `refresh` and `save`
        // only once) to avoid having to apply the changeset here.
        changeset
            .apply(
                &mut tx,
                self.branch().id(),
                self.branch()
                    .keys()
                    .write()
                    .ok_or(Error::PermissionDenied)?,
            )
            .await?;

        let branch_id = *self.branch().id();
        let mut changeset = Changeset::new();
        let src_content = self
            .begin_remove_entry(
                &mut tx,
                &mut changeset,
                src_name,
                &branch_id,
                src_vv,
                TombstoneCause::Moved,
            )
            .await?;

        self.commit(tx, changeset).await?;
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

        // Because we are transferring only the directory but not its content, we reflect that
        // by setting its version vector to what the version vector of the source directory was
        // at the time it was initially created - that is, it's current version vector minus the
        // version vectors of all its entries.

        let (parent, initial_vv) = {
            // Running this in a read transaction to make sure the version vector fo this directory
            // and the version vectors of its entries are in sync.

            let mut tx = self.branch().store().begin_read().await?;

            let (parent, self_vv) = if let Some(parent) = &self.parent {
                let parent_dir = parent.open_in(&mut tx, self.branch().clone()).await?;
                let entry_name = parent.entry_name();
                let self_vv = parent_dir.lookup(entry_name)?.version_vector().clone();

                (Some((parent_dir, entry_name)), self_vv)
            } else {
                let self_vv = tx
                    .load_root_node(self.branch().id(), RootNodeFilter::Any)
                    .await?
                    .proof
                    .into_version_vector();

                (None, self_vv)
            };

            let (_, content) = self.load(&mut tx, DirectoryFallback::Disabled).await?;

            let entries_vv = content
                .iter()
                .map(|(_, entry)| entry.version_vector())
                .sum();
            let initial_vv = self_vv.saturating_sub(&entries_vv);

            (parent, initial_vv)
        };

        if let Some((parent_dir, entry_name)) = parent {
            let mut parent_dir = parent_dir.fork(local_branch).await?;
            parent_dir
                .open_or_create_directory(entry_name, initial_vv)
                .await
        } else {
            Self::open_or_create_root(local_branch.clone(), initial_vv).await
        }
    }

    pub(crate) async fn parent(&self) -> Result<Option<Directory>> {
        if let Some(parent) = &self.parent {
            Ok(Some(parent.open(self.branch().clone()).await?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn is_root(&self) -> bool {
        self.parent.is_none()
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
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
        name: &str,
        branch_id: &PublicKey,
        version_vector: VersionVector,
        cause: TombstoneCause,
    ) -> Result<Content> {
        let mut new_data = match self.lookup(name) {
            Ok(old_entry)
                if branch_id != self.branch().id()
                    && old_entry
                        .version_vector()
                        .partial_cmp(&version_vector)
                        .is_none() =>
            {
                let mut new_data = old_entry.clone_data();
                new_data.version_vector_mut().merge(&version_vector);
                new_data
            }
            Ok(_) | Err(Error::EntryNotFound) => {
                EntryData::Tombstone(EntryTombstoneData::new(cause, version_vector))
            }
            Err(error) => return Err(error),
        };

        new_data.version_vector_mut().increment(*self.branch().id());

        self.begin_insert_entry(tx, changeset, name.to_owned(), new_data)
            .await
    }

    async fn check_directory_empty(&self, tx: &mut ReadTransaction, name: &str) -> Result<()> {
        match self.lookup(name) {
            Ok(EntryRef::Directory(entry)) => {
                if entry
                    .open_snapshot(tx, DirectoryFallback::Disabled)
                    .await?
                    .iter()
                    .all(|(_, data)| matches!(data, EntryData::Tombstone(_)))
                {
                    Ok(())
                } else {
                    Err(Error::DirectoryNotEmpty)
                }
            }
            Ok(_) | Err(Error::EntryNotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn begin_insert_entry(
        &mut self,
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
        name: String,
        data: EntryData,
    ) -> Result<Content> {
        self.refresh_in(tx).await?;

        let mut content = self.content.clone();
        let diff = content.insert(name, data)?;
        self.save(tx, changeset, &content).await?;
        self.bump(tx, changeset, Bump::Add(diff)).await?;

        Ok(content)
    }

    async fn refresh_in(&mut self, tx: &mut ReadTransaction) -> Result<()> {
        if self.blob.is_dirty() {
            Ok(())
        } else {
            let (blob, content) = self.load(tx, DirectoryFallback::Disabled).await?;
            self.blob = blob;
            self.content = content;

            Ok(())
        }
    }

    async fn load(
        &self,
        tx: &mut ReadTransaction,
        fallback: DirectoryFallback,
    ) -> Result<(Blob, Content)> {
        load(tx, self.branch().clone(), *self.blob_id(), fallback).await
    }

    async fn save(
        &mut self,
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
        content: &Content,
    ) -> Result<()> {
        // Save the directory content into the store
        let buffer = content.serialize();
        self.blob.truncate(0)?;
        self.blob.write_all(tx, changeset, &buffer).await?;
        self.blob.flush(tx, changeset).await?;

        Ok(())
    }

    /// Atomically commits the transaction and sends notification event.
    async fn commit(&mut self, tx: WriteTransaction, changeset: Changeset) -> Result<()> {
        commit(tx, changeset, self.branch()).await
    }

    /// Updates the version vectors of this directory and all its ancestors.
    #[async_recursion]
    async fn bump(
        &mut self,
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
        bump: Bump,
    ) -> Result<()> {
        // Update the version vector of this directory and all it's ancestors
        if let Some(parent) = self.parent.as_mut() {
            parent
                .bump(tx, changeset, self.blob.branch().clone(), bump)
                .await
        } else {
            changeset.bump(bump);
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

/// Update the root version vector of the given branch by merging it with `merge`.
/// If `merge` is less that or equal to the current root version vector, this is s no-op.
#[instrument(skip(branch), fields(writer_id = ?branch.id()))]
pub(crate) async fn bump_root(branch: &Branch, merge: VersionVector) -> Result<()> {
    let tx = branch.store().begin_write().await?;
    let mut changeset = Changeset::new();
    changeset.bump(Bump::Merge(merge));
    commit(tx, changeset, branch).await
}

// Load directory content. On missing block, fallback to previous snapshot (if any).
async fn load(
    tx: &mut ReadTransaction,
    branch: Branch,
    blob_id: BlobId,
    fallback: DirectoryFallback,
) -> Result<(Blob, Content)> {
    let mut root_node = tx.load_root_node(branch.id(), RootNodeFilter::Any).await?;

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

/// Apply the changeset, commit the transaction and send a notification event.
async fn commit(mut tx: WriteTransaction, changeset: Changeset, branch: &Branch) -> Result<()> {
    let changed = changeset
        .apply(
            &mut tx,
            branch.id(),
            branch.keys().write().ok_or(Error::PermissionDenied)?,
        )
        .await?;

    if !changed {
        return Ok(());
    }

    let event_tx = branch.notify();
    tx.commit_and_then(move || event_tx.send()).await?;

    Ok(())
}
