use super::{cache::SubdirectoryCache, entry_data::EntryData, parent_context::ParentContext};
use crate::{
    blob::{Blob, Shared},
    blob_id::BlobId,
    branch::Branch,
    crypto::sign::PublicKey,
    db,
    error::{Error, Result},
    locator::Locator,
    sync::RwLockWriteGuard,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use std::{
    cmp::Ordering,
    collections::{btree_map, BTreeMap},
    sync::Weak,
};

pub(super) struct Inner {
    pub blob: Blob,
    // map of entry name to map of author to entry data
    pub entries: BTreeMap<String, BTreeMap<PublicKey, EntryData>>,
    // have `entries` been modified since the last `flush`?
    pub dirty: bool,
    pub parent: Option<ParentContext>,
    // Cache of open subdirectories. Used to make sure that multiple instances of the same directory
    // all share the same internal state.
    pub open_directories: SubdirectoryCache,
}

impl Inner {
    pub async fn flush(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }

        let buffer =
            bincode::serialize(&self.entries).expect("failed to serialize directory content");

        self.blob.truncate_in_transaction(tx, 0).await?;
        self.blob.write_in_transaction(tx, &buffer).await?;
        self.blob.flush_in_transaction(tx).await?;

        self.dirty = false;

        Ok(())
    }

    // If `keep` is set to Some(some_blob_id), than that blob won't be removed from the store. This
    // is useful when we want to move or rename a an entry.
    pub async fn insert_entry(
        &mut self,
        name: String,
        author: PublicKey,
        mut new_data: EntryData,
        keep: Option<BlobId>,
    ) -> Result<()> {
        let versions = self.entries.entry(name).or_insert_with(Default::default);
        let mut old_blob_ids = Vec::new();

        let new_data = match versions.entry(author) {
            btree_map::Entry::Vacant(entry) => entry.insert(new_data),
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

                old_blob_ids.extend(old_data.blob_id().copied());
                *old_data = new_data;

                old_data
            }
        };

        // Remove outdated versions
        let new_vv = new_data.version_vector().clone();
        versions.retain(|_, data| {
            if data.version_vector() < &new_vv {
                old_blob_ids.extend(data.blob_id().copied());
                false
            } else {
                true
            }
        });

        self.dirty = true;

        // Remove blobs of the outdated versions.
        // TODO: This should succeed/fail atomically with the above.
        // TODO: when GC is implemented, this won't be necessary.
        remove_outdated_blobs(self.blob.branch(), old_blob_ids, keep).await?;

        Ok(())
    }

    /// Inserts a file entry into this directory. It's the responsibility of the caller to make
    /// sure the passed in `blob_id` eventually points to an actual file.
    pub async fn insert_file_entry(
        &mut self,
        name: String,
        author_id: PublicKey,
        version_vector: VersionVector,
        blob_id: BlobId,
    ) -> Result<()> {
        let data = EntryData::file(blob_id, version_vector, Weak::new());
        self.insert_entry(name, author_id, data, None).await
    }

    // Modify an entry in this directory with the specified name and author.
    pub fn modify_entry(
        &mut self,
        name: &str,
        author_id: &mut PublicKey,
        increment: &VersionVector,
    ) -> Result<()> {
        let local_id = *self.branch().id();
        let versions = self.entries.get_mut(name).ok_or(Error::EntryNotFound)?;

        if !can_overwrite(versions, author_id, &local_id) {
            return Err(Error::EntryExists);
        }

        let mut version = versions.remove(author_id).ok_or(Error::EntryNotFound)?;
        *version.version_vector_mut() += increment;
        versions.insert(local_id, version);

        *author_id = local_id;
        self.dirty = true;

        Ok(())
    }

    // Modify the entry of this directory in its parent.
    pub async fn modify_self_entry(
        &mut self,
        tx: db::Transaction<'_>,
        increment: &VersionVector,
    ) -> Result<()> {
        if let Some(ctx) = self.parent.as_mut() {
            ctx.modify_entry(tx, increment).await
        } else {
            let write_keys = self
                .blob
                .branch()
                .keys()
                .write()
                .ok_or(Error::PermissionDenied)?;

            self.blob
                .branch()
                .data()
                .update_root_version_vector(tx, increment, write_keys)
                .await
        }
    }

    pub fn entry_version_vector(&self, name: &str, author: &PublicKey) -> Option<&VersionVector> {
        Some(self.entries.get(name)?.get(author)?.version_vector())
    }

    fn branch(&self) -> &Branch {
        self.blob.branch()
    }
}

fn can_overwrite(
    versions: &BTreeMap<PublicKey, EntryData>,
    lhs: &PublicKey,
    rhs: &PublicKey,
) -> bool {
    if lhs == rhs {
        return true;
    }

    let lhs_vv = if let Some(vv) = versions.get(lhs).map(|v| v.version_vector()) {
        vv
    } else {
        return true;
    };

    let rhs_vv = if let Some(vv) = versions.get(rhs).map(|v| v.version_vector()) {
        vv
    } else {
        return true;
    };

    lhs_vv.partial_cmp(rhs_vv) == Some(Ordering::Greater)
}

#[async_recursion]
pub(super) async fn modify_entry<'a>(
    mut tx: db::Transaction<'a>,
    inner: RwLockWriteGuard<'a, Inner>,
    name: &'a str,
    author_id: &'a mut PublicKey,
    increment: &'a VersionVector,
) -> Result<()> {
    let mut op = ModifyEntry::new(inner, name, author_id);
    op.apply(increment)?;
    op.inner.flush(&mut tx).await?;
    op.inner.modify_self_entry(tx, increment).await?;
    op.commit();

    Ok(())
}

/// Helper for the `modify_entry` that allows to undo the operation in case of error.
struct ModifyEntry<'a> {
    inner: RwLockWriteGuard<'a, Inner>,
    name: &'a str,
    author_id: &'a mut PublicKey,
    orig_author_id: PublicKey,
    orig_versions: Option<BTreeMap<PublicKey, EntryData>>,
    orig_dirty: bool,
    committed: bool,
}

impl<'a> ModifyEntry<'a> {
    fn new(
        inner: RwLockWriteGuard<'a, Inner>,
        name: &'a str,
        author_id: &'a mut PublicKey,
    ) -> Self {
        let orig_author_id = *author_id;
        let orig_versions = inner.entries.get(name).cloned();
        let orig_dirty = inner.dirty;

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
    fn apply(&mut self, increment: &VersionVector) -> Result<()> {
        self.inner
            .modify_entry(self.name, self.author_id, increment)
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
        self.inner.dirty = self.orig_dirty;

        if let Some(versions) = self.orig_versions.take() {
            // `unwrap` is OK here because the existence of `versions_backup` implies that the
            // entry for `name` exists because we never remove it during the `modify_entry` call.
            // Also as the `ModifyEntry` struct is holding an exclusive lock (write lock) to the
            // directory internals, it's impossible for someone to remove the entry in the meantime.
            *self.inner.entries.get_mut(self.name).unwrap() = versions;
        } else {
            self.inner.entries.remove(self.name);
        }
    }
}

async fn remove_outdated_blobs(
    branch: &Branch,
    remove: Vec<BlobId>,
    keep: Option<BlobId>,
) -> Result<()> {
    for blob_id in remove {
        if Some(blob_id) == keep {
            continue;
        }

        Blob::open(
            branch.clone(),
            Locator::head(blob_id),
            Shared::uninit().into(),
        )
        .await?
        .remove()
        .await?;
    }

    Ok(())
}
