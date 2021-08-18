use crate::{
    directory::{Directory, EntryData},
    error::Result,
};
use std::sync::Arc;

/// Context needed for updating all necessary info when writing to a file or directory.
pub struct WriteContext {
    // None iff this WriteContext corresponds to the root directory.
    parent: Option<ParentContext>,
}

#[derive(Clone)]
pub struct ParentContext {
    pub directory: Directory,
    pub entry_name: String,
    pub entry_data: Arc<EntryData>,
    // TODO: Should this be std::sync::Weak?
}

impl ParentContext {
    /// Increment the version of the entry.
    pub async fn increment_version(&self) -> Result<()> {
        self.directory
            .increment_entry_version(&self.entry_name)
            .await
    }
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
            parent: Some(ParentContext {
                directory: parent_directory,
                entry_name,
                entry_data,
            }),
        }
    }

    pub fn parent(&self) -> Option<&ParentContext> {
        self.parent.as_ref()
    }
}
