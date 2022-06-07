use super::{
    cache::SubdirectoryCache,
    content::{self, Content},
    entry_data::EntryData,
    parent_context::ParentContext,
};
use crate::{
    blob::{Blob, Shared},
    block,
    branch::Branch,
    db,
    error::{Error, Result},
    locator::Locator,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use std::io::SeekFrom;

pub(super) struct Inner {
    pub blob: Blob,
    pub parent: Option<ParentContext>,
    // Cache of open subdirectories. Used to make sure that multiple instances of the same directory
    // all share the same internal state.
    pub open_directories: SubdirectoryCache,

    entries: Content,
    pending_entry: Option<PendingEntry>,
}

impl Inner {
    pub fn create(owner_branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        let blob = Blob::create(owner_branch, locator, Shared::uninit());

        Self {
            blob,
            parent,
            open_directories: SubdirectoryCache::new(),
            entries: Default::default(),
            pending_entry: None,
        }
    }

    pub async fn open(
        owner_branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
    ) -> Result<Self> {
        let mut blob = Blob::open(owner_branch, locator, Shared::uninit().into()).await?;
        let buffer = blob.read_to_end().await?;
        let entries = content::deserialize(&buffer)?;

        Ok(Self {
            blob,
            parent,
            open_directories: SubdirectoryCache::new(),
            entries,
            pending_entry: None,
        })
    }

    pub fn entries(&self) -> &Content {
        &self.entries
    }

    /// Prepare this directory and its ancestors for modification. This checks whether there are
    /// any previous uncommited changes (which would indicate an earlier operation failed or was
    /// cancelled) and if so, reloads the content from the db.
    #[async_recursion]
    pub async fn prepare(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        if let Some(parent) = &self.parent {
            parent.directory().write().await.inner.prepare(tx).await?;
        }

        if self.pending_entry.is_some() {
            self.blob
                .seek_in_transaction(tx, SeekFrom::Start(0))
                .await?;
            let buffer = self.blob.read_to_end_in_connection(tx).await?;
            self.entries = content::deserialize(&buffer)?;
        }

        // Note: To ensure cancel safety, `pending_entry` is cleared only at the end of `commit`
        // (after `tx` is commited).

        Ok(())
    }

    /// Saves the content of this directory into the db and removes overwritten blobs, if any.
    pub async fn save(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        // Remove overwritten blob
        if let Some(pending) = self.pending_entry.as_ref() {
            if matches!(pending.overwrite, OverwriteStrategy::Remove) {
                if let Some(old_blob_id) = self
                    .entries
                    .get(&pending.name)
                    .and_then(|data| data.blob_id())
                {
                    Blob::remove_in_transaction(tx, self.blob.branch(), Locator::head(*old_blob_id))
                        .await?
                }
            }
        }

        // Save the directory content into the store
        let buffer = content::serialize(
            &self.entries,
            self.pending_entry
                .as_ref()
                .map(|pending| (pending.name.as_str(), &pending.data)),
        );
        self.blob.truncate_in_transaction(tx, 0).await?;
        self.blob.write_in_transaction(tx, &buffer).await?;
        self.blob.flush_in_transaction(tx).await?;

        Ok(())
    }

    /// Atomically commits any pending changes in this directory and updates the version vectors of
    /// it and all its ancestors.
    #[async_recursion]
    pub async fn commit<'a>(
        &'a mut self,
        mut tx: db::Transaction<'a>,
        merge: VersionVector,
    ) -> Result<()> {
        // Update the version vector of this directory and all it's ancestors
        if let Some(ctx) = self.parent.as_mut() {
            ctx.modify_entry(tx, merge).await?;
        } else {
            // At this point all local newly created blocks should become reachable so they can be
            // safely unpinned to become normal subjects of garbage collection.
            block::unpin_all(&mut tx).await?;

            let write_keys = self
                .blob
                .branch()
                .keys()
                .write()
                .ok_or(Error::PermissionDenied)?;

            self.blob
                .branch()
                .data()
                .update_root_version_vector(tx, &merge, write_keys)
                .await?;
        }

        if let Some(pending) = self.pending_entry.take() {
            self.entries.insert(pending.name, pending.data);
        }

        Ok(())
    }

    // If `overwrite` is set to `Keep`, the existing blob won't be removed from the store. This is
    // useful when we want to move or rename an entry.
    pub fn insert_entry(
        &mut self,
        name: String,
        mut new_data: EntryData,
        overwrite: OverwriteStrategy,
    ) -> Result<()> {
        if let Some(old_data) = self.entries.get(&name) {
            match old_data {
                EntryData::File(_) | EntryData::Directory(_)
                    if new_data.version_vector() > old_data.version_vector() => {}
                EntryData::File(_) | EntryData::Directory(_) => {
                    // Don't allow overwriting existing entry unless it is strictly older than
                    // the new entry.
                    return Err(Error::EntryExists);
                }
                EntryData::Tombstone(old_data) => {
                    new_data
                        .version_vector_mut()
                        .merge(&old_data.version_vector);
                }
            }
        }

        self.pending_entry = Some(PendingEntry {
            name,
            data: new_data,
            overwrite,
        });

        Ok(())
    }

    /// Updates the version vector of entry at `name`. The version vector is updated by merging it
    /// with `merge` and incrementing the local version.
    pub fn modify_entry(&mut self, name: &str, merge: &VersionVector) -> Result<()> {
        let mut data = self.entries.get(name).ok_or(Error::EntryNotFound)?.clone();

        data.version_vector_mut().merge(merge);
        data.version_vector_mut()
            .increment(*self.blob.branch().id());

        self.pending_entry = Some(PendingEntry {
            name: name.to_owned(),
            data,
            overwrite: OverwriteStrategy::Keep,
        });

        Ok(())
    }

    pub fn db_pool(&self) -> &db::Pool {
        self.blob.db_pool()
    }
}

/// What to do with the existing entry when inserting a new entry in its place.
pub(crate) enum OverwriteStrategy {
    // Remove it
    Remove,
    // Keep it (useful when inserting a tombstone oven an entry which is to be moved somewhere
    // else)
    Keep,
}

struct PendingEntry {
    name: String,
    data: EntryData,
    overwrite: OverwriteStrategy,
}

// fn bump(local_id: &PublicKey, old: &mut VersionVector, new: &VersionVector) {

// }
