use super::{inner::OverwriteStrategy, Mode};
use crate::{
    blob::Blob, blob_id::BlobId, branch::Branch, db, directory::Directory, error::Result,
    locator::Locator, version_vector::VersionVector,
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

    /// Atomically finalizes any pending modifications to the entry that holds this parent context.
    /// This updates the version vector of this entry and all its ancestors, flushes them and
    /// finally commits the transaction.
    pub async fn commit(
        &self,
        mut tx: db::Transaction<'_>,
        branch: Branch,
        merge: VersionVector,
    ) -> Result<()> {
        let mut directory = self.directory(&mut tx, branch).await?;
        let inner = directory.inner.get_mut();
        let mut content = inner.entries.clone();
        content.bump(inner.branch(), &self.entry_name, &merge)?;
        inner
            .save(&mut tx, &content, OverwriteStrategy::Keep)
            .await?;
        inner.commit(tx, content, merge).await?;

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
        mut tx: db::Transaction<'_>,
        entry_blob: &Blob,
        src_branch: Branch,
        dst_branch: Branch,
    ) -> Result<(Self, Blob)> {
        let directory = self.directory(&mut tx, src_branch).await?;
        let entry_data = directory
            .read()
            .await
            .lookup(&self.entry_name)?
            .clone_data();

        assert_eq!(entry_data.blob_id(), Some(entry_blob.locator().blob_id()));

        let mut directory = directory.fork(&mut tx, &dst_branch).await?;
        let inner = directory.inner.get_mut();

        let mut content = inner.entries.clone();
        content.insert(inner.branch(), self.entry_name.clone(), entry_data)?;
        inner
            .save(&mut tx, &content, OverwriteStrategy::Remove)
            .await?;
        let new_blob = entry_blob.try_fork(&mut tx, dst_branch).await?;
        inner.commit(tx, content, VersionVector::new()).await?;

        let directory_id = *inner.blob_id();
        let parent = inner.parent.clone();

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

    /// Returns the parent directory of this entry.
    pub async fn directory(&self, conn: &mut db::Connection, branch: Branch) -> Result<Directory> {
        Directory::open(
            conn,
            branch,
            Locator::head(self.directory_id),
            self.parent.as_deref().cloned(),
            Mode::ReadWrite,
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
            .directory(conn, branch)
            .await?
            .read()
            .await
            .lookup(&self.entry_name)?
            .version_vector()
            .clone())
    }
}
