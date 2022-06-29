use super::{
    cache::SubdirectoryCache,
    content::{self, Content},
    entry_data::EntryData,
    parent_context::ParentContext,
};
use crate::{
    blob::{Blob, Shared},
    branch::Branch,
    db,
    error::{Error, Result},
    locator::Locator,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use std::collections::btree_map::Entry;

pub(super) struct Inner {
    pub blob: Blob,
    pub parent: Option<ParentContext>,
    // Cache of open subdirectories. Used to make sure that multiple instances of the same directory
    // all share the same internal state.
    pub open_directories: SubdirectoryCache,

    entries: Content,
}

impl Inner {
    pub fn create(owner_branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        let blob = Blob::create(owner_branch, locator, Shared::uninit());

        Self {
            blob,
            parent,
            open_directories: SubdirectoryCache::new(),
            entries: Default::default(),
        }
    }

    pub async fn open(
        conn: &mut db::Connection,
        owner_branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
    ) -> Result<Self> {
        let (blob, entries) = load(conn, owner_branch, locator).await?;

        Ok(Self {
            blob,
            parent,
            open_directories: SubdirectoryCache::new(),
            entries,
        })
    }

    pub fn entries(&self) -> &Content {
        &self.entries
    }

    pub async fn load(&mut self, conn: &mut db::Connection) -> Result<Content> {
        if self.blob.is_dirty() {
            Ok(self.entries.clone())
        } else {
            let (blob, content) =
                load(conn, self.blob.branch().clone(), *self.blob.locator()).await?;
            self.blob = blob;
            Ok(content)
        }
    }

    pub async fn save(
        &mut self,
        tx: &mut db::Transaction<'_>,
        content: &Content,
        overwrite: OverwriteStrategy,
    ) -> Result<()> {
        // Remove overwritten blob
        if matches!(overwrite, OverwriteStrategy::Remove) {
            for blob_id in content::overwritten(&self.entries, content) {
                match Blob::remove(tx, self.branch(), Locator::head(*blob_id)).await {
                    // If we get `EntryNotFound` or `BlockNotFound` it most likely means the
                    // blob is already removed which can legitimately happen due to several
                    // reasons so we don't treat it as an error.
                    Ok(()) | Err(Error::EntryNotFound | Error::BlockNotFound(_)) => (),
                    Err(error) => return Err(error),
                }
            }
        }

        // Save the directory content into the store
        let buffer = content::serialize_2(content);
        self.blob.truncate(tx, 0).await?;
        self.blob.write(tx, &buffer).await?;
        self.blob.flush(tx).await?;

        Ok(())
    }

    /// Atomically commits any pending changes in this directory and updates the version vectors of
    /// it and all its ancestors.
    #[async_recursion]
    pub async fn commit<'a>(
        &'a mut self,
        tx: db::Transaction<'a>,
        content: Content,
        bump: VersionVector,
    ) -> Result<()> {
        // Update the version vector of this directory and all it's ancestors
        if let Some(ctx) = self.parent.as_mut() {
            ctx.commit(tx, bump).await?;
        } else {
            let write_keys = self
                .branch()
                .keys()
                .write()
                .ok_or(Error::PermissionDenied)?;

            self.branch().data().bump(tx, &bump, write_keys).await?;
        }

        if !content.is_empty() {
            self.entries = content;
        }

        Ok(())
    }

    pub fn branch(&self) -> &Branch {
        self.blob.branch()
    }
}

pub(super) fn insert(
    content: &mut Content,
    branch: &Branch,
    name: String,
    mut new_data: EntryData,
) -> Result<()> {
    match content.entry(name) {
        Entry::Vacant(entry) => {
            entry.insert(new_data);
        }
        Entry::Occupied(mut entry) => {
            match entry.get() {
                // Overwrite files only if the new version is more up to date than the old version
                // and the old version is not currently open.
                EntryData::File(old_data)
                    if new_data.version_vector() > &old_data.version_vector
                        && !branch.is_blob_open(&old_data.blob_id) => {}
                // Overwrite directories only if the new version is more up to date than the old
                // version.
                EntryData::Directory(old_data)
                    if new_data.version_vector() > &old_data.version_vector => {}
                EntryData::File(_) | EntryData::Directory(_) => return Err(Error::EntryExists),
                // Always overwrite tombstones but update the new version vector so it's more up to
                // date than the tombstone.
                EntryData::Tombstone(old_data) => {
                    let mut vv = old_data.version_vector.clone();
                    vv.bump(new_data.version_vector(), branch.id());
                    *new_data.version_vector_mut() = vv;
                }
            }

            entry.insert(new_data);
        }
    }

    Ok(())
}

/// Updates the version vector of entry at `name`.
pub(super) fn bump(
    content: &mut Content,
    branch: &Branch,
    name: &str,
    bump: &VersionVector,
) -> Result<()> {
    content
        .get_mut(name)
        .ok_or(Error::EntryNotFound)?
        .version_vector_mut()
        .bump(bump, branch.id());

    Ok(())
}

async fn load(
    conn: &mut db::Connection,
    branch: Branch,
    locator: Locator,
) -> Result<(Blob, Content)> {
    let mut blob = Blob::open(conn, branch, locator, Shared::uninit()).await?;
    let buffer = blob.read_to_end(conn).await?;
    let content = content::deserialize(&buffer)?;

    Ok((blob, content))
}

/// What to do with the existing entry when inserting a new entry in its place.
pub(crate) enum OverwriteStrategy {
    // Remove it
    Remove,
    // Keep it (useful when inserting a tombstone oven an entry which is to be moved somewhere
    // else)
    Keep,
}
