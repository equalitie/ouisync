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
    blob::Blob,
    branch::Branch,
    crypto::sign::PublicKey,
    db,
    debug::DebugPrinter,
    error::{Error, Result},
    file::File,
    index::{SnapshotData, VersionVectorOp},
    locator::Locator,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use std::{fmt, mem};

#[derive(Clone)]
pub struct Directory {
    blob: Blob,
    parent: Option<ParentContext>,
    entries: Content,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens the root directory.
    /// For internal use only. Use [`Branch::open_root`] instead.
    pub(crate) async fn open_root(
        branch: Branch,
        missing_block_strategy: MissingBlockStrategy,
    ) -> Result<Self> {
        Self::open(branch, Locator::ROOT, None, missing_block_strategy).await
    }

    /// Opens the root directory or creates it if it doesn't exist.
    /// For internal use only. Use [`Branch::open_or_create_root`] instead.
    pub(crate) async fn open_or_create_root(branch: Branch) -> Result<Self> {
        // TODO: make sure this is atomic
        let locator = Locator::ROOT;

        match Self::open(branch.clone(), locator, None, MissingBlockStrategy::Fail).await {
            Ok(dir) => Ok(dir),
            Err(Error::EntryNotFound) => Ok(Self::create(branch, locator, None)),
            Err(error) => Err(error),
        }
    }

    /// Reloads this directory from the db.
    pub(crate) async fn refresh(&mut self) -> Result<()> {
        let Self {
            blob,
            parent,
            entries,
        } = Self::open(
            self.branch().clone(),
            *self.locator(),
            self.parent.as_ref().cloned(),
            MissingBlockStrategy::Fail,
        )
        .await?;

        self.blob = blob;
        self.parent = parent;
        self.entries = entries;

        Ok(())
    }

    /// Lookup an entry of this directory by name.
    pub fn lookup(&self, name: &'_ str) -> Result<EntryRef> {
        self.entries
            .get_key_value(name)
            .map(|(name, data)| EntryRef::new(self, name, data))
            .ok_or(Error::EntryNotFound)
    }

    /// Returns iterator over the entries of this directory.
    pub fn entries(&self) -> impl Iterator<Item = EntryRef> + DoubleEndedIterator + Clone {
        self.entries
            .iter()
            .map(move |(name, data)| EntryRef::new(self, name, data))
    }

    /// Creates a new file inside this directory.
    pub async fn create_file(&mut self, name: String) -> Result<File> {
        let mut tx = self.branch().db().begin().await?;
        let mut content = self.load(&mut tx).await?;

        let blob_id = rand::random();
        let version_vector = content
            .initial_version_vector(&name)
            .incremented(*self.branch().id());
        let data = EntryData::file(blob_id, version_vector);
        let parent =
            ParentContext::new(*self.locator().blob_id(), name.clone(), self.parent.clone());

        let mut file = File::create(self.branch().clone(), Locator::head(blob_id), parent);

        content.insert(self.branch(), name, data)?;
        file.save(&mut tx).await?;
        self.save(&mut tx, &content).await?;
        self.commit(tx, content, &VersionVectorOp::IncrementLocal)
            .await?;

        Ok(file)
    }

    /// Creates a new subdirectory of this directory.
    pub async fn create_directory(&mut self, name: String) -> Result<Self> {
        self.create_directory_with_version_vector_op(name, &VersionVectorOp::IncrementLocal)
            .await
    }

    async fn create_directory_with_version_vector_op(
        &mut self,
        name: String,
        op: &VersionVectorOp,
    ) -> Result<Self> {
        let mut tx = self.branch().db().begin().await?;
        let mut content = self.load(&mut tx).await?;

        let blob_id = rand::random();

        let mut version_vector = content.initial_version_vector(&name);
        op.apply(self.branch().id(), &mut version_vector);

        let data = EntryData::directory(blob_id, version_vector);
        let parent =
            ParentContext::new(*self.locator().blob_id(), name.clone(), self.parent.clone());

        let mut dir =
            Directory::create(self.branch().clone(), Locator::head(blob_id), Some(parent));

        content.insert(self.branch(), name, data)?;
        dir.save(&mut tx, &Content::empty()).await?;
        self.save(&mut tx, &content).await?;
        self.commit(tx, content, op).await?;

        Ok(dir)
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
        let mut tx = self.branch().db().begin().await?;

        let content = match self
            .begin_remove_entry(&mut tx, name, branch_id, tombstone)
            .await
        {
            Ok(content) => content,
            // `EntryExists` in this case means the tombstone already exists which means the entry
            // is already removed.
            Err(Error::EntryExists) => return Ok(()),
            Err(error) => return Err(error),
        };

        tx.commit().await?;
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

        let mut tx = self.branch().db().begin().await?;

        let dst_content = dst_dir
            .begin_insert_entry(&mut tx, dst_name.to_owned(), dst_data)
            .await?;

        let branch_id = *self.branch().id();
        let src_content = self
            .begin_remove_entry(
                &mut tx,
                src_name,
                &branch_id,
                EntryTombstoneData::moved(src_vv),
            )
            .await?;

        tx.commit().await?;

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

            match parent_dir.lookup(entry_name) {
                Ok(EntryRef::Directory(entry)) => {
                    let mut dir = entry.open(MissingBlockStrategy::Fail).await?;
                    dir.merge_version_vector(vv).await?;
                    Ok(dir)
                }
                Ok(EntryRef::File(_)) => {
                    // TODO: return some kind of `Error::Conflict`
                    Err(Error::EntryIsFile)
                }
                Ok(EntryRef::Tombstone(_)) | Err(Error::EntryNotFound) => {
                    parent_dir
                        .create_directory_with_version_vector_op(
                            entry_name.to_owned(),
                            &VersionVectorOp::Merge(vv),
                        )
                        .await
                }
                Err(error) => Err(error),
            }
        } else {
            local_branch.open_or_create_root().await
        }
    }

    /// Updates the version vector of this directory by merging it with `vv`.
    pub(crate) async fn merge_version_vector(&mut self, vv: VersionVector) -> Result<()> {
        let tx = self.branch().db().begin().await?;
        self.commit(tx, Content::empty(), &VersionVectorOp::Merge(vv))
            .await
    }

    pub async fn parent(&self) -> Result<Option<Directory>> {
        if let Some(parent) = &self.parent {
            Ok(Some(parent.open(self.branch().clone()).await?))
        } else {
            Ok(None)
        }
    }

    async fn open_in(
        conn: &mut db::Connection,
        branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
        missing_block_strategy: MissingBlockStrategy,
    ) -> Result<Self> {
        let (blob, entries) = load(conn, branch, locator, missing_block_strategy).await?;

        Ok(Self {
            blob,
            parent,
            entries,
        })
    }

    async fn open(
        branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
        missing_block_strategy: MissingBlockStrategy,
    ) -> Result<Self> {
        let mut conn = branch.db().acquire().await?;
        Self::open_in(&mut conn, branch, locator, parent, missing_block_strategy).await
    }

    fn create(owner_branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        let blob = Blob::create(owner_branch, locator);

        Directory {
            blob,
            parent,
            entries: Content::empty(),
        }
    }

    #[async_recursion]
    pub async fn debug_print(&self, print: DebugPrinter) {
        for (name, entry_data) in &self.entries {
            print.display(&format_args!("{:?}: {:?}", name, entry_data));

            match entry_data {
                EntryData::File(file_data) => {
                    let print = print.indent();

                    let parent_context = ParentContext::new(
                        *self.locator().blob_id(),
                        name.into(),
                        self.parent.clone(),
                    );

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

                    let parent_context = ParentContext::new(
                        *self.locator().blob_id(),
                        name.into(),
                        self.parent.clone(),
                    );

                    let dir = Directory::open(
                        self.blob.branch().clone(),
                        Locator::head(data.blob_id),
                        Some(parent_context),
                        MissingBlockStrategy::Fail,
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

    /// Locator of this directory
    pub(crate) fn locator(&self) -> &Locator {
        self.blob.locator()
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
        tx: &mut db::Transaction,
        name: &str,
        branch_id: &PublicKey,
        mut tombstone: EntryTombstoneData,
    ) -> Result<Content> {
        // If we are removing a directory, ensure it's empty (recursive removal can still be
        // implemented at the upper layers).
        if matches!(tombstone.cause, TombstoneCause::Removed) {
            match self.lookup(name) {
                Ok(EntryRef::Directory(entry)) => {
                    if entry
                        .open_in(tx, MissingBlockStrategy::Fail)
                        .await?
                        .entries()
                        .any(|entry| !entry.is_tombstone())
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
        tx: &mut db::Transaction,
        name: String,
        data: EntryData,
    ) -> Result<Content> {
        let mut content = self.load(tx).await?;
        content.insert(self.branch(), name, data)?;
        self.save(tx, &content).await?;
        self.bump(tx, &VersionVectorOp::IncrementLocal).await?;

        Ok(content)
    }

    async fn load(&mut self, conn: &mut db::Connection) -> Result<Content> {
        if self.blob.is_dirty() {
            Ok(self.entries.clone())
        } else {
            let (blob, content) = load(
                conn,
                self.branch().clone(),
                *self.locator(),
                MissingBlockStrategy::Fail,
            )
            .await?;
            self.blob = blob;
            Ok(content)
        }
    }

    async fn save(&mut self, tx: &mut db::Transaction, content: &Content) -> Result<()> {
        // Save the directory content into the store
        let buffer = content.serialize();
        self.blob.truncate(tx, 0).await?;
        self.blob.write(tx, &buffer).await?;
        self.blob.flush(tx).await?;

        Ok(())
    }

    /// Atomically commits any pending changes in this directory and updates the version vectors of
    /// it and all its ancestors.
    async fn commit(
        &mut self,
        mut tx: db::Transaction,
        content: Content,
        op: &VersionVectorOp,
    ) -> Result<()> {
        self.bump(&mut tx, op).await?;
        tx.commit().await?;
        self.finalize(content);

        Ok(())
    }

    /// Updates the version vectors of this directory and all its ancestors.
    #[async_recursion]
    async fn bump(&mut self, tx: &mut db::Transaction, op: &VersionVectorOp) -> Result<()> {
        // Update the version vector of this directory and all it's ancestors
        if let Some(parent) = self.parent.as_mut() {
            parent.bump(tx, self.blob.branch().clone(), op).await
        } else {
            let write_keys = self
                .branch()
                .keys()
                .write()
                .ok_or(Error::PermissionDenied)?;

            self.branch().data().bump(tx, op, write_keys).await
        }
    }

    /// Finalize pending modifications. Call this only after the db transaction is committed.
    fn finalize(&mut self, content: Content) {
        if !content.is_empty() {
            self.entries = content;
        }

        self.branch().data().notify();
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
            .field("blob_id", self.locator().blob_id())
            .finish()
    }
}

/// What to do when opening a directory with a missing block.
#[derive(Clone, Copy)]
pub enum MissingBlockStrategy {
    /// Attempt to fallback to the previous snapshot
    Fallback,
    /// Fail the whole open operation
    Fail,
}

// Load directory content. On missing block, fallback to previous snapshot (if any).
async fn load(
    conn: &mut db::Connection,
    branch: Branch,
    locator: Locator,
    missing_block_strategy: MissingBlockStrategy,
) -> Result<(Blob, Content)> {
    let mut snapshot = branch.data().load_snapshot(conn).await?;

    loop {
        let error = match load_in(conn, branch.clone(), &snapshot, locator).await {
            Ok((blob, content)) => return Ok((blob, content)),
            Err(error @ Error::BlockNotFound(_)) => error,
            Err(error) => return Err(error),
        };

        match missing_block_strategy {
            MissingBlockStrategy::Fallback => (),
            MissingBlockStrategy::Fail => return Err(error),
        }

        if let Some(prev_snapshot) = snapshot.load_prev(conn).await? {
            snapshot = prev_snapshot;
        } else {
            return Err(error);
        }
    }
}

async fn load_in(
    conn: &mut db::Connection,
    branch: Branch,
    snapshot: &SnapshotData,
    locator: Locator,
) -> Result<(Blob, Content)> {
    let mut blob = Blob::open_in(conn, branch, snapshot, locator).await?;
    let buffer = blob.read_to_end_in(conn, snapshot).await?;
    let content = Content::deserialize(&buffer)?;

    Ok((blob, content))
}
