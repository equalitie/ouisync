use crate::{
    blob::Blob,
    branch::Branch,
    directory::{Directory, EntryData},
    error::Result,
    locator::Locator,
};
use async_recursion::async_recursion;
use std::sync::Arc;

/// Context needed for updating all necessary info when writing to a file or directory.
pub struct WriteContext {
    // None iff this WriteContext corresponds to the root directory.
    parent: Option<Parent>,
}

#[derive(Clone)]
struct Parent {
    directory: Directory,
    entry_name: String,
    entry_data: Arc<EntryData>,
    // TODO: Should this be std::sync::Weak?
}

impl WriteContext {
    pub fn root() -> Self {
        Self { parent: None }
    }

    pub async fn child(
        &self,
        parent_directory: Directory,
        entry_name: String,
        entry_data: Arc<EntryData>,
    ) -> Self {
        Self {
            parent: Some(Parent {
                directory: parent_directory,
                entry_name,
                entry_data,
            }),
        }
    }

    /// Ensure the blob lives in the local branch and all its ancestor directories exist and live
    /// in the local branch as well. Should be called before
    pub async fn begin(&mut self, local_branch: &Branch, blob: &mut Blob) -> Result<()> {
        if blob.branch().id() == local_branch.id() {
            // Blob already lives in the local branch. We assume the ancestor directories have been
            // already created as well so there is nothing else to do.
            return Ok(());
        }

        let dst_locator = if let Some(parent) = &mut self.parent {
            parent.directory = fork_directory(&parent.directory, local_branch).await?;
            parent.entry_data = parent
                .directory
                .insert_entry(
                    parent.entry_name.clone(),
                    parent.entry_data.entry_type(),
                    parent.entry_data.version_vector().clone(),
                )
                .await?;

            parent.entry_data.locator()
        } else {
            // `blob` is the root directory.
            Locator::Root
        };

        blob.fork(local_branch.clone(), dst_locator).await
    }

    /// Commit writing to the blob started by a previous call to `begin`.
    ///
    /// # Panics
    ///
    /// Panics if called without `begin` being called first.
    // TODO: consider rewriting this to not use recursion.
    #[async_recursion]
    pub async fn commit(&self) -> Result<()> {
        if let Some(parent) = &self.parent {
            parent
                .directory
                .increment_entry_version(&parent.entry_name)
                .await?;
            parent.directory.apply().await?;
            parent
                .directory
                .read()
                .await
                .write_context()
                .commit()
                .await?;
        }

        Ok(())
    }

    pub fn parent_directory(&self) -> Option<&Directory> {
        self.parent.as_ref().map(|parent| &parent.directory)
    }
}

// TODO: consider rewriting this to not use recursion.
#[async_recursion]
async fn fork_directory(dir: &Directory, local_branch: &Branch) -> Result<Directory> {
    if let Some(parent) = &dir.read().await.write_context().parent {
        let parent_dir = fork_directory(&parent.directory, local_branch).await?;
        let parent_dir_reader = parent_dir.read().await;

        if let Ok(entry) = parent_dir_reader.lookup_version(&parent.entry_name, local_branch.id()) {
            entry.directory()?.open().await
        } else {
            parent_dir
                .create_directory(parent.entry_name.to_string())
                .await
        }
    } else {
        local_branch.open_or_create_root().await
    }
}
