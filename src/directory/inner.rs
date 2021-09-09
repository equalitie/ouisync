use super::{cache::SubdirectoryCache, parent_context::ParentContext};
use crate::{
    blob::{self, Blob},
    blob_id::BlobId,
    db,
    entry_type::EntryType,
    error::{Error, Result},
    locator::Locator,
    replica_id::ReplicaId,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use futures_util::future;
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map, BTreeMap},
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, RwLockWriteGuard};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct EntryData {
    pub entry_type: EntryType,
    pub blob_id: BlobId,
    pub version_vector: VersionVector,
    #[serde(skip)]
    pub blob_core: Arc<Mutex<Weak<Mutex<blob::Core>>>>,
}

impl PartialEq for EntryData {
    fn eq(&self, other: &Self) -> bool {
        self.entry_type == other.entry_type
            && self.blob_id == other.blob_id
            && self.version_vector == other.version_vector
    }
}

impl Eq for EntryData {}

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

    pub async fn insert_entry(
        &mut self,
        name: String,
        author: ReplicaId,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<BlobId> {
        let (new_blob_id, old_blob_ids) =
            self.content
                .insert(name, author, entry_type, version_vector)?;

        // TODO: This should succeed/fail atomically with the above.
        let branch = self.blob.branch();
        future::try_join_all(old_blob_ids.into_iter().map(|old_blob_id| async move {
            Blob::open(branch.clone(), Locator::Head(old_blob_id))
                .await?
                .remove()
                .await
        }))
        .await?;

        Ok(new_blob_id)
    }

    // Modify an entry in this directory with the specified name and author.
    pub fn modify_entry(
        &mut self,
        name: &str,
        author_id: &mut ReplicaId,
        local_id: ReplicaId,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        let versions = self
            .content
            .entries
            .get_mut(name)
            .ok_or(Error::EntryNotFound)?;
        let authors_version = versions.get(author_id).ok_or(Error::EntryNotFound)?;

        if *author_id != local_id {
            // There may already exist a local version of the entry. If it does, we may
            // overwrite it only if the existing version "happened before" this new one being
            // modified.  Note that if there doesn't alreay exist a local version, that is
            // essentially the same as if it did exist but it's version_vector was a zero
            // vector.
            let local_version = versions.get(&local_id);
            let local_happened_before = local_version.map_or(true, |local_version| {
                local_version.version_vector < authors_version.version_vector
            });

            // TODO: use a more descriptive error here.
            if !local_happened_before {
                return Err(Error::EntryExists);
            }
        }

        // `unwrap` is OK because we already established the entry exists.
        let mut version = versions.remove(author_id).unwrap();

        if let Some(version_vector_override) = version_vector_override {
            version.version_vector.merge(version_vector_override)
        } else {
            version.version_vector.increment(local_id);
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
        local_id: ReplicaId,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        if let Some(ctx) = self.parent.as_mut() {
            ctx.modify_entry(tx, local_id, version_vector_override)
                .await
        } else {
            self.blob
                .branch()
                .data()
                .update_root_version_vector(tx, version_vector_override)
                .await
        }
    }

    pub fn entry_version_vector(&self, name: &str, author: &ReplicaId) -> Option<&VersionVector> {
        Some(&self.content.entries.get(name)?.get(author)?.version_vector)
    }

    #[track_caller]
    pub fn assert_local(&self, local_id: &ReplicaId) {
        assert_eq!(
            self.blob.branch().id(),
            local_id,
            "mutable operations not allowed - directory is not in the local branch"
        )
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Content {
    pub entries: BTreeMap<String, BTreeMap<ReplicaId, EntryData>>,
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
    /// Returns the blob id of the newly inserted entry and the blob ids of all the removed entries
    /// (if any). It's the responsibility of the caller to remove the old blobs.
    fn insert(
        &mut self,
        name: String,
        author: ReplicaId,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<(BlobId, Vec<BlobId>)> {
        let blob_id = rand::random();
        let versions = self.entries.entry(name).or_insert_with(Default::default);

        // Find outdated entries
        // clippy: false positive - the iterator borrows a value that is subsequently mutated, so
        // the `collect` is needed to work around that.
        #[allow(clippy::needless_collect)]
        let old_authors: Vec<_> = versions
            .iter()
            .filter(|(_, data)| data.version_vector < version_vector)
            .map(|(id, _)| *id)
            .collect();

        match versions.entry(author) {
            btree_map::Entry::Vacant(entry) => {
                entry.insert(EntryData {
                    entry_type,
                    blob_id,
                    version_vector,
                    blob_core: Arc::new(Mutex::new(Weak::new())),
                });
            }
            btree_map::Entry::Occupied(mut entry) => {
                let data = entry.get_mut();
                if data.entry_type != entry_type {
                    return Err(Error::EntryExists);
                }

                // If the existing entry is
                //     1. "same", or
                //     2. "happens after", or
                //     3. "concurrent"
                // then don't update it. Note that #3 should not happen because of the invariant
                // that one replica (version author) must not create concurrent entries.
                #[allow(clippy::neg_cmp_op_on_partial_ord)]
                if !(data.version_vector < version_vector) {
                    return Err(Error::EntryExists);
                }

                data.blob_id = blob_id;
                data.version_vector = version_vector;
            }
        }

        // Remove the outdated entries and collect their blob ids.
        let old_blob_ids = old_authors
            .into_iter()
            .filter(|old_author| *old_author != author)
            .filter_map(|old_author| versions.remove(&old_author))
            .map(|data| data.blob_id)
            .collect();

        self.dirty = true;

        Ok((blob_id, old_blob_ids))
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        self.entries.remove(name).ok_or(Error::EntryNotFound)?;
        self.dirty = true;

        Ok(())
    }
}

#[async_recursion]
pub(super) async fn modify_entry<'a>(
    mut tx: db::Transaction<'a>,
    inner: RwLockWriteGuard<'a, Inner>,
    local_id: ReplicaId,
    name: &'a str,
    author_id: &'a mut ReplicaId,
    version_vector_override: Option<&'a VersionVector>,
) -> Result<()> {
    inner.assert_local(&local_id);

    let mut op = ModifyEntry::new(inner, name, author_id);
    op.apply(local_id, version_vector_override)?;
    op.inner.flush(&mut tx).await?;
    op.inner
        .modify_self_entry(tx, local_id, version_vector_override)
        .await?;
    op.commit();

    Ok(())
}

/// Helper for the `modify_entry` that allows to undo the operation in case of error.
struct ModifyEntry<'a> {
    inner: RwLockWriteGuard<'a, Inner>,
    name: &'a str,
    author_id: &'a mut ReplicaId,
    orig_author_id: ReplicaId,
    orig_versions: Option<BTreeMap<ReplicaId, EntryData>>,
    orig_dirty: bool,
    committed: bool,
}

impl<'a> ModifyEntry<'a> {
    fn new(
        inner: RwLockWriteGuard<'a, Inner>,
        name: &'a str,
        author_id: &'a mut ReplicaId,
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
    fn apply(
        &mut self,
        local_id: ReplicaId,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        self.inner
            .modify_entry(self.name, self.author_id, local_id, version_vector_override)
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
