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
pub struct Parent {
    pub directory: Directory,
    pub entry_name: String,
    entry_data: Arc<EntryData>,
    // TODO: Should this be std::sync::Weak?
}

impl WriteContext {
    /// Write context for the root directory.
    pub const ROOT: Self = Self { parent: None };

    /// Write context for a child entry of the given parent directory.
    pub fn child(
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
    pub async fn ensure_local(&mut self, local_branch: &Branch, blob: &mut Blob) -> Result<()> {
        if blob.branch().id() == local_branch.id() {
            // Blob already lives in the local branch. We assume the ancestor directories have been
            // already created as well so there is nothing else to do.
            return Ok(());
        }

        let dst_locator = if let Some(parent) = &mut self.parent {
            parent.directory = parent.directory.fork().await?;
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

    /// Increment the version of the blob and all its ancestor directories.
    ///
    /// # Panics
    ///
    /// Panics if the ancestor directories are not in the local branch (call `ensure_local` first
    /// to avoid)
    // TODO: consider rewriting this to not use recursion.
    #[async_recursion]
    pub async fn increment_version(&self) -> Result<()> {
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
                .increment_version()
                .await?;
        }

        Ok(())
    }

    pub fn parent(&self) -> Option<&Parent> {
        self.parent.as_ref()
    }

    pub fn parent_directory(&self) -> Option<&Directory> {
        self.parent.as_ref().map(|parent| &parent.directory)
    }
}
