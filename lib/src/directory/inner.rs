use super::{
    cache::SubdirectoryCache,
    content::{self, Content},
    entry_data::EntryData,
    parent_context::ParentContext,
};
use crate::{
    blob::{Blob, Shared},
    blob_id::BlobId,
    block,
    branch::Branch,
    db,
    error::{Error, Result},
    locator::Locator,
    sync::RwLockWriteGuard,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use std::{collections::btree_map, mem, sync::Weak};

pub(super) struct Inner {
    pub blob: Blob,
    pub entries: Content,
    // If this is an empty version vector it means this directory hasn't been modified. Otherwise
    // the version vector of this directory will be incremented by this on the next `flush`.
    pub version_vector_increment: VersionVector,
    pub parent: Option<ParentContext>,
    // Cache of open subdirectories. Used to make sure that multiple instances of the same directory
    // all share the same internal state.
    pub open_directories: SubdirectoryCache,
}

impl Inner {
    pub async fn flush(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        let buffer = content::serialize(&self.entries);
        self.blob.truncate_in_transaction(tx, 0).await?;
        self.blob.write_in_transaction(tx, &buffer).await?;
        self.blob.flush_in_transaction(tx).await?;

        Ok(())
    }

    // If `overwrite` is set to `Keep`, the existing blob won't be removed from the store. This is
    // useful when we want to move or rename a an entry.
    pub async fn insert_entry(
        &mut self,
        name: String,
        mut new_data: EntryData,
        overwrite: OverwriteStrategy,
    ) -> Result<()> {
        let (old_vv, new_vv) = match self.entries.entry(name) {
            btree_map::Entry::Vacant(entry) => (
                VersionVector::new(),
                entry.insert(new_data).version_vector().clone(),
            ),
            btree_map::Entry::Occupied(entry) => {
                let old_data = entry.into_mut();

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

                if matches!(overwrite, OverwriteStrategy::Remove) {
                    if let Some(blob_id) = old_data.blob_id() {
                        remove_blob(self.blob.branch(), *blob_id).await?;
                    }
                }

                let new_vv = new_data.version_vector().clone();

                let old_data = mem::replace(old_data, new_data);
                let old_vv = old_data.into_version_vector();

                (old_vv, new_vv)
            }
        };

        self.version_vector_increment += &(new_vv - old_vv);

        Ok(())
    }

    /// Inserts a file entry into this directory. It's the responsibility of the caller to make
    /// sure the passed in `blob_id` eventually points to an actual file.
    pub async fn insert_file_entry(
        &mut self,
        name: String,
        version_vector: VersionVector,
        blob_id: BlobId,
    ) -> Result<()> {
        let data = EntryData::file(blob_id, version_vector, Weak::new());
        self.insert_entry(name, data, OverwriteStrategy::Remove)
            .await
    }

    // Modify an entry in this directory with the specified name.
    pub fn modify_entry(&mut self, name: &str, increment: &VersionVector) -> Result<()> {
        let data = self.entries.get_mut(name).ok_or(Error::EntryNotFound)?;
        *data.version_vector_mut() += increment;
        self.version_vector_increment += increment;

        Ok(())
    }

    // Modify the entry of this directory in its parent.
    pub async fn modify_self_entry(&mut self, mut tx: db::Transaction<'_>) -> Result<()> {
        let increment = mem::take(&mut self.version_vector_increment);

        if let Some(ctx) = self.parent.as_mut() {
            ctx.modify_entry(tx, &increment).await
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
                .update_root_version_vector(tx, &increment, write_keys)
                .await?;

            Ok(())
        }
    }

    pub fn entry_version_vector(&self, name: &str) -> Option<&VersionVector> {
        Some(self.entries.get(name)?.version_vector())
    }
}

#[async_recursion]
pub(super) async fn modify_entry<'a>(
    mut tx: db::Transaction<'a>,
    inner: RwLockWriteGuard<'a, Inner>,
    name: &'a str,
    increment: &'a VersionVector,
) -> Result<()> {
    let mut op = ModifyEntry::new(inner, name);
    op.apply(increment)?;
    op.inner.flush(&mut tx).await?;
    op.inner.modify_self_entry(tx).await?;
    op.commit();

    Ok(())
}

/// Helper for the `modify_entry` that allows to undo the operation in case of error.
struct ModifyEntry<'a> {
    inner: RwLockWriteGuard<'a, Inner>,
    name: &'a str,
    orig_data: Option<EntryData>,
    orig_version_vector_increment: VersionVector,
    committed: bool,
}

impl<'a> ModifyEntry<'a> {
    fn new(inner: RwLockWriteGuard<'a, Inner>, name: &'a str) -> Self {
        let orig_data = inner.entries.get(name).cloned();
        let orig_version_vector_increment = inner.version_vector_increment.clone();

        Self {
            inner,
            name,
            orig_data,
            orig_version_vector_increment,
            committed: false,
        }
    }

    // Apply the operation. The operation can still be undone after this by dropping `self`.
    fn apply(&mut self, increment: &VersionVector) -> Result<()> {
        self.inner.modify_entry(self.name, increment)
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

        self.inner.version_vector_increment = mem::take(&mut self.orig_version_vector_increment);

        if let Some(data) = self.orig_data.take() {
            self.inner.entries.insert(self.name.to_owned(), data);
        } else {
            self.inner.entries.remove(self.name);
        }
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

async fn remove_blob(branch: &Branch, id: BlobId) -> Result<()> {
    Blob::open(branch.clone(), Locator::head(id), Shared::uninit().into())
        .await?
        .remove()
        .await
}
