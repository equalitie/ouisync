use super::{
    entry::EntryRef,
    entry_data::EntryData,
    inner::{self, OverwriteStrategy},
};
use crate::{
    branch::Branch, db, directory::Directory, error::Result, version_vector::VersionVector,
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
    /// If an error occurs anywhere in the process, all intermediate changes are rolled back and all
    /// the affected directories are reverted to their state before calling this function.
    pub async fn modify_entry(
        &mut self,
        tx: db::Transaction<'_>,
        increment: &VersionVector,
    ) -> Result<()> {
        inner::modify_entry(
            tx,
            self.directory.inner.write().await,
            &self.entry_name,
            increment,
        )
        .await
    }

    /// Forks this parent context by forking the parent directory and inserting the forked entry
    /// data into it.
    pub async fn fork(&self, local_branch: &Branch) -> Result<Self> {
        let entry_data = self.fork_entry_data().await;
        let directory = self.directory().fork(local_branch).await?;

        directory.inner.write().await.insert_entry(
            self.entry_name.clone(),
            entry_data,
            OverwriteStrategy::Remove,
        )?;

        Ok(Self {
            directory,
            entry_name: self.entry_name.clone(),
        })
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
        self.map_entry(|entry| entry.fork_data()).await
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
