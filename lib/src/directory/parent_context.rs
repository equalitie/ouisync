use super::{entry::EntryRef, entry_data::EntryData, inner::OverwriteStrategy};
use crate::{
    blob::Blob, branch::Branch, db, directory::Directory, error::Result,
    version_vector::VersionVector,
};

/// Info about an entry in the context of its parent directory.
#[derive(Clone)]
pub(crate) struct ParentContext {
    /// The parent directory of the entry.
    directory: Directory,
    /// The name of the entry in its parent directory.
    entry_name: String,
}

impl ParentContext {
    pub(super) fn new(directory: Directory, entry_name: String) -> Self {
        Self {
            directory,
            entry_name,
        }
    }

    /// Atomically finalizes any pending modifications to the entry that holds this parent context.
    /// This updates the version vector of this entry and all its ancestors, flushes them and
    /// finally commits the transaction.
    ///
    /// TODO: document cancel safety.
    pub async fn commit(&self, mut tx: db::Transaction<'_>, merge: VersionVector) -> Result<()> {
        let mut writer = self.directory.write().await?;
        writer.inner.prepare(&mut tx).await?;
        writer.inner.bump(&self.entry_name, &merge)?;
        writer.inner.save(&mut tx).await?;
        writer.inner.commit(tx, merge).await
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
        local_branch: &Branch,
    ) -> Result<(Self, Blob)> {
        let entry_data = self.fork_entry_data().await;
        assert_eq!(entry_data.blob_id(), Some(entry_blob.locator().blob_id()));

        let directory = self.directory.fork(&mut tx, local_branch).await?;
        let mut writer = directory.write().await?;

        writer.inner.prepare(&mut tx).await?;
        writer.inner.insert(
            self.entry_name.clone(),
            entry_data,
            OverwriteStrategy::Remove,
        )?;
        writer.inner.save(&mut tx).await?;
        let new_blob = entry_blob.try_fork(&mut tx, local_branch.clone()).await?;
        writer.inner.commit(tx, VersionVector::new()).await?;

        drop(writer);

        let new_context = Self {
            directory,
            entry_name: self.entry_name.clone(),
        };

        Ok((new_context, new_blob))
    }

    pub(super) fn entry_name(&self) -> &str {
        &self.entry_name
    }

    /// Returns the parent directory of the entry bound to the given local branch.
    pub fn directory(&self) -> &Directory {
        &self.directory
    }

    /// Returns the version vector of this entry.
    ///
    /// # Panics
    ///
    /// Panics if this `ParentContext` doesn't correspond to any existing entry in the parent
    /// directory.
    pub async fn entry_version_vector(&self) -> VersionVector {
        self.map_entry(|entry| entry.version_vector().clone()).await
    }

    async fn fork_entry_data(&self) -> EntryData {
        self.map_entry(|entry| entry.clone_data()).await
    }

    async fn map_entry<F, R>(&self, f: F) -> R
    where
        F: FnOnce(EntryRef) -> R,
    {
        f(self
            .directory
            .read()
            .await
            .lookup(&self.entry_name)
            .expect("dangling ParentContext"))
    }
}
