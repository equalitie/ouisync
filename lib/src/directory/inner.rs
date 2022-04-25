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
use futures_util::future;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{btree_map, BTreeMap},
};

pub(super) struct Inner {
    pub blob: Blob,
    pub content: Content,
    pub parent: Option<ParentContext>,
    // Cache of open subdirectories. Used to make sure that multiple instances of the same directory
    // all share the same internal state.
    pub open_directories: SubdirectoryCache,
}

impl Inner {
    pub async fn flush(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        if !self.content.dirty {
            return Ok(());
        }

        let buffer =
            bincode::serialize(&self.content).expect("failed to serialize directory content");

        self.blob.truncate_in_transaction(tx, 0).await?;
        self.blob.write_in_transaction(tx, &buffer).await?;
        self.blob.flush_in_transaction(tx).await?;

        self.content.dirty = false;

        Ok(())
    }

    // If `keep` is set to Some(some_blob_id), than that blob won't be removed from the store. This
    // is useful when we want to move or rename a an entry.
    pub async fn insert_entry(
        &mut self,
        name: String,
        author: PublicKey,
        entry_data: EntryData,
        keep: Option<BlobId>,
    ) -> Result<()> {
        let old_blobs = self.content.insert(name.clone(), author, entry_data)?;

        // TODO: This should succeed/fail atomically with the above.
        let branch = self.blob.branch();

        let to_delete = old_blobs.into_iter().filter(|b| keep != Some(*b));

        future::try_join_all(to_delete.map(|old_blob_id| async move {
            Blob::open(
                branch.clone(),
                Locator::head(old_blob_id),
                Shared::uninit().into(),
            )
            .await?
            .remove()
            .await
        }))
        .await?;

        Ok(())
    }

    // Modify an entry in this directory with the specified name and author.
    pub fn modify_entry(
        &mut self,
        name: &str,
        author_id: &mut PublicKey,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        let local_id = *self.branch().id();
        let versions = self
            .content
            .entries
            .get_mut(name)
            .ok_or(Error::EntryNotFound)?;

        if !can_overwrite(versions, author_id, &local_id) {
            return Err(Error::EntryExists);
        }

        let mut version = versions.remove(author_id).ok_or(Error::EntryNotFound)?;

        if let Some(version_vector_override) = version_vector_override {
            version.version_vector_mut().merge(version_vector_override)
        } else {
            version.version_vector_mut().increment(local_id);
        }

        versions.insert(local_id, version);

        *author_id = local_id;
        self.content.dirty = true;

        Ok(())
    }

    // Modify the entry of this directory in its parent.
    pub async fn modify_self_entry(
        &mut self,
        tx: db::Transaction<'_>,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        if let Some(ctx) = self.parent.as_mut() {
            ctx.modify_entry(tx, version_vector_override).await
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
                .update_root_version_vector(tx, version_vector_override, write_keys)
                .await
        }
    }

    pub fn entry_version_vector(&self, name: &str, author: &PublicKey) -> Option<&VersionVector> {
        Some(
            self.content
                .entries
                .get(name)?
                .get(author)?
                .version_vector(),
        )
    }

    fn branch(&self) -> &Branch {
        self.blob.branch()
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub(super) struct Content {
    pub entries: BTreeMap<String, BTreeMap<PublicKey, EntryData>>,
    #[serde(skip)]
    pub dirty: bool,
}

impl Content {
    pub fn new() -> Self {
        Self {
            entries: Default::default(),
            dirty: true,
        }
    }

    /// Inserts entry into the content and removes all previously existing versions whose version
    /// vector "happens before" the new entry. If an entry with the same `author` already exists
    /// and its version vector is not "happens before" the new entry, nothing is inserted or
    /// removed and an error is returned instead.
    ///
    /// Returns the blob ids of all the removed entries (if any). It's the responsibility of the
    /// caller to remove the old blobs.
    fn insert(
        &mut self,
        name: String,
        author: PublicKey,
        new_data: EntryData,
    ) -> Result<Vec<BlobId>> {
        let versions = self.entries.entry(name).or_insert_with(Default::default);

        // Find outdated entries
        // clippy: false positive - the iterator borrows a value that is subsequently mutated, so
        // the `collect` is needed to work around that.
        #[allow(clippy::needless_collect)]
        let old_authors: Vec<_> = versions
            .iter()
            // We'll check `author`'s VersionVector below, so no need to do it twice
            .filter(|(id, _)| *id != &author)
            .filter(|(_, old_data)| old_data.version_vector() < new_data.version_vector())
            .map(|(id, _)| *id)
            .collect();

        let old_blob_id = match versions.entry(author) {
            btree_map::Entry::Vacant(entry) => {
                entry.insert(new_data);
                None
            }
            btree_map::Entry::Occupied(mut entry) => {
                let old_data = entry.get_mut();

                // If the existing entry is
                //     1. "same", or
                //     2. "happens after", or
                //     3. "concurrent"
                // then don't update it. Note that #3 should not happen because of the invariant
                // that one replica (version author) must not create concurrent entries.
                #[allow(clippy::neg_cmp_op_on_partial_ord)]
                if !(old_data.version_vector() < new_data.version_vector()) {
                    return Err(Error::EntryExists);
                }

                match (&*old_data, &new_data) {
                    (EntryData::File(_), EntryData::Directory(_))
                    | (EntryData::Directory(_), EntryData::File(_)) => {
                        return Err(Error::EntryExists)
                    }
                    _ => (),
                }

                let old_blob_id = match old_data {
                    EntryData::File(data) => Some(data.blob_id),
                    EntryData::Directory(data) => Some(data.blob_id),
                    EntryData::Tombstone(_) => None,
                };

                *old_data = new_data;
                old_blob_id
            }
        };

        // Remove the outdated entries and collect their blob ids.
        let old_blobs = old_authors
            .into_iter()
            .filter(|old_author| *old_author != author)
            .filter_map(|old_author| versions.remove(&old_author))
            .filter_map(|data| match data {
                EntryData::File(data) => Some(data.blob_id),
                EntryData::Directory(data) => Some(data.blob_id),
                EntryData::Tombstone(_) => None,
            })
            // Because we filtered out *old_author != author above.
            .chain(old_blob_id)
            .collect();

        self.dirty = true;

        Ok(old_blobs)
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

    match lhs_vv.partial_cmp(rhs_vv) {
        Some(Ordering::Greater) => true,
        Some(Ordering::Equal | Ordering::Less) | None => false,
    }
}

#[async_recursion]
pub(super) async fn modify_entry<'a>(
    mut tx: db::Transaction<'a>,
    inner: RwLockWriteGuard<'a, Inner>,
    name: &'a str,
    author_id: &'a mut PublicKey,
    version_vector_override: Option<&'a VersionVector>,
) -> Result<()> {
    let mut op = ModifyEntry::new(inner, name, author_id);
    op.apply(version_vector_override)?;
    op.inner.flush(&mut tx).await?;
    op.inner
        .modify_self_entry(tx, version_vector_override)
        .await?;
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
        let orig_versions = inner.content.entries.get(name).cloned();
        let orig_dirty = inner.content.dirty;

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
    fn apply(&mut self, version_vector_override: Option<&VersionVector>) -> Result<()> {
        self.inner
            .modify_entry(self.name, self.author_id, version_vector_override)
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
        self.inner.content.dirty = self.orig_dirty;

        if let Some(versions) = self.orig_versions.take() {
            // `unwrap` is OK here because the existence of `versions_backup` implies that the
            // entry for `name` exists because we never remove it during the `modify_entry` call.
            // Also as the `ModifyEntry` struct is holding an exclusive lock (write lock) to the
            // directory internals, it's impossible for someone to remove the entry in the meantime.
            *self.inner.content.entries.get_mut(self.name).unwrap() = versions;
        } else {
            self.inner.content.entries.remove(self.name);
        }
    }
}
