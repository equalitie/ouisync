use std::cmp::Ordering;

use super::{Error, OverwriteStrategy};
use crate::{
    blob::Blob,
    blob_id::BlobId,
    branch::Branch,
    db,
    directory::{content::EntryExists, Directory},
    error::Result,
    index::VersionVectorOp,
    locator::Locator,
    version_vector::VersionVector,
};

/// Info about an entry in the context of its parent directory.
#[derive(Clone)]
pub(crate) struct ParentContext {
    /// BlobId of the parent directory of the entry.
    directory_id: BlobId,
    /// The name of the entry in its parent directory.
    entry_name: String,
    // ParentContext of the parent directory ("grandparent context")
    parent: Option<Box<Self>>,
}

impl ParentContext {
    pub(super) fn new(directory_id: BlobId, entry_name: String, parent: Option<Self>) -> Self {
        Self {
            directory_id,
            entry_name,
            parent: parent.map(Box::new),
        }
    }

    /// This updates the version vector of this entry and all its ancestors.
    pub async fn bump(
        &self,
        tx: &mut db::Transaction<'_>,
        branch: Branch,
        op: &VersionVectorOp,
    ) -> Result<()> {
        let mut directory = self.open_in(tx, branch).await?;
        let mut content = directory.entries.clone();
        content.bump(directory.branch(), &self.entry_name, op)?;
        directory
            .save(tx, &content, OverwriteStrategy::Keep)
            .await?;
        directory.bump(tx, op).await?;

        Ok(())
    }

    /// Atomically forks the blob into the local branch and returns it together with its updated
    /// parent context.
    ///
    /// # Panics
    ///
    /// Panics if `entry_blob` is not the blob of the entry corresponding to this parent context.
    ///
    pub async fn fork(
        &self,
        conn: &mut db::PoolConnection,
        entry_blob: &Blob,
        src_branch: Branch,
        dst_branch: Branch,
    ) -> Result<(Self, Blob)> {
        let directory = self.open_in(conn, src_branch).await?;
        let src_entry_data = directory.lookup(&self.entry_name)?.clone_data();

        assert_eq!(
            src_entry_data.blob_id(),
            Some(entry_blob.locator().blob_id())
        );

        let mut directory = directory.fork(conn, &dst_branch).await?;
        let mut content = directory.entries.clone();
        let src_vv = src_entry_data.version_vector().clone();

        let mut tx = conn.begin().await?;

        let new_blob =
            match content.insert(directory.branch(), self.entry_name.clone(), src_entry_data) {
                Ok(()) => {
                    directory
                        .save(&mut tx, &content, OverwriteStrategy::Remove)
                        .await?;
                    let new_blob = entry_blob.try_fork(&mut tx, dst_branch).await?;
                    directory
                        .commit(tx, content, &VersionVectorOp::Merge(src_vv))
                        .await?;
                    new_blob
                }
                Err(EntryExists { new, old }) => {
                    // It's possible that another task has already forked this entry. If that's the
                    // case then we return success.
                    let blob_id = *entry_blob.locator().blob_id();

                    if Some(&blob_id) != old.blob_id() {
                        return Err(Error::EntryExists);
                    }

                    match new.version_vector().partial_cmp(old.version_vector()) {
                        Some(Ordering::Less | Ordering::Equal) => (),
                        Some(Ordering::Greater) | None => return Err(Error::EntryExists),
                    }

                    tx.commit().await?;

                    Blob::open(conn, dst_branch, Locator::head(blob_id)).await?
                }
            };

        let directory_id = *directory.locator().blob_id();
        let parent = directory.parent.clone();

        let new_context = Self {
            directory_id,
            entry_name: self.entry_name.clone(),
            parent: parent.map(Box::new),
        };

        Ok((new_context, new_blob))
    }

    pub(super) fn entry_name(&self) -> &str {
        &self.entry_name
    }

    /// Opens the parent directory of this entry.
    pub async fn open_in(&self, conn: &mut db::Connection, branch: Branch) -> Result<Directory> {
        Directory::open_in(
            conn,
            branch,
            Locator::head(self.directory_id),
            self.parent.as_deref().cloned(),
        )
        .await
    }

    /// Returns the version vector of this entry.
    ///
    /// # Panics
    ///
    /// Panics if this `ParentContext` doesn't correspond to any existing entry in the parent
    /// directory.
    pub async fn entry_version_vector(
        &self,
        conn: &mut db::Connection,
        branch: Branch,
    ) -> Result<VersionVector> {
        Ok(self
            .open_in(conn, branch)
            .await?
            .lookup(&self.entry_name)?
            .version_vector()
            .clone())
    }
}
